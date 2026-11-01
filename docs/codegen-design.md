# Codegen v1 design

Issue #31, milestone 2 ("first frame"). How `codegen` turns a ROM's
discovered functions (decision 5) and their IR (decision 3,
`analysis::ir`) into a Rust crate in `out/<name>/` that runs on `runtime`.
Decisions 1, 2, 8, 10, 11 and 12 apply; the sprint brief (#36) fixes the
output as naive: `loop { match }` blocks, every register in `cpu.r[]`,
every flag computed. Flag liveness and const-prop (decision 9) have since
been added before printing; structuring (Ramsey) has since replaced the
`loop { match }` for every ROM function with a checked plan
([structured-codegen-design.md](structured-codegen-design.md), step 8;
`--no-structured` keeps this form), and ABI trust comes later. All are
measured against this baseline.

The runtime core (#26), timing and IRQs (#27), BIOS HLE (#28) and the
desktop (#29) are other threads. What codegen needs from them is listed as
requirements at the end; nothing here is runtime code.

## The one invariant

**All guest state lives in `Machine` at every block boundary.** Registers in
`cpu.r[]`, flags in the CPSR, memory on the bus. A generated Rust frame
holds nothing but `pc`, the guest address it is about to run. So:

- any host frame of function F may run any block of F, and it doesn't
  matter which frame does it;
- unwinding any number of host frames back to the dispatcher is always
  correct, only slower (the frames' callers are later re-entered at their
  return sites through the dispatcher);
- the interpreter can take over at any block boundary, and hand back at
  any block start.

Every choice below leans on this. It is the part ABI-trust mode (decision
10) has to rework, by writing locals back at each of the same boundaries.

## Addresses: the key

Block labels, dispatcher keys and `Flow` targets are guest addresses in
interworking form, as discovery already uses: bit 0 set for Thumb. A Thumb
block at `0x0800_0124` is `0x0800_0125`; an ARM one is `0x0800_0124`. One
function's `match` may hold blocks of both instruction sets (discovery's
`Kind::Entry` roots, `adr r1, 1f; bx r1`).

The key's bit 0 always equals the CPSR's T bit. Generated code never sets
T itself except through `Cpu::apply_exit` (below), so the two can't drift.
`cpu.pc` is *not* kept current inside generated code; it is exact whenever
control is outside it (in the dispatcher, the interpreter, a `swi`, or after
`run_frame` returns), because every `Flow` carries the pc and the
dispatcher writes it back.

In ARM state the interpreter keeps a jump target's low bits (`mov pc, r0`
with `r0 & 3 != 0`, following NanoBoyAdvance). Exits that can do that go
through `Cpu::apply_exit`, so `cpu.pc` keeps them; only the key is aligned.
Known deviation: when such a key goes back through the dispatcher,
`set_pc` writes the aligned address. No compiler emits that.

## The shape of a generated function

One Rust fn per guest function (decision 8), named by its entry key:

```rust
pub fn f_08000101(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0101 => {
                insn!(0x0800_0100, "Block { load: false, .. }"); // push {r4, lr}
                let v1 = m.cpu.r[13];
                let v2 = v1.wrapping_sub(8);
                let v3 = m.cpu.r[4];
                m.bus.store(Width::Word, v2 & !3, v3);
                // ...
                insn!(0x0800_0104, "ThumbBlHi { .. }");
                m.cpu.r[14] = 0x0800_0108;
                insn!(0x0800_0106, "ThumbBlLo { .. }");
                m.cpu.r[14] = 0x0800_0109;
                if m.tick(5) {
                    return Flow::Jump(0x0800_0201);
                }
                let Flow::Jump(a) = f_08000201(m, 0x0800_0201);
                pc = a;
                continue;
            }
            0x0800_0109 => {
                // ... adds r0, r4; pop {r4}; pop {r1}
                insn!(0x0800_010e, "Bx { rm: R1, .. }");
                let v20 = m.cpu.r[1];
                pc = exit(m, 0x0800_010e, true, Exit::Exchange(v20));
                if m.tick(7) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}
```

(Illustrative; the field spellings follow whatever #26 and #27 land.)

- **The second argument is the block to start at.** Callers pass the entry
  key; the dispatcher passes any block start of the function. That is how a
  return site, a jump-table target or an IRQ's return point is re-entered
  after an unwind, with no separate entry stubs.
- **The `_` arm is the only way out** apart from the tick yield. Anything the
  function doesn't own (a return to the caller, a tail call, an unresolved
  jump, a jump into the BIOS) goes back up as `Flow::Jump(key)`.
- **Block bodies are the IR printed in order, with as few statements and
  locals as the IR allows** (`codegen::print`; "Printing a block" below).
  Values that need a name get `let vN = ...;`, effects are plain
  statements. A guarded effect becomes `if vG { ... }`, one `if` for a run
  under the same guard; a guarded `Load` becomes
  `let vN = if vG { load } else { 0 };` (the IR only uses it under the same
  guard). Bits are `bool`. Before printing, `build_block` runs const-prop
  and DCE and `build_func` flag liveness (decision 9), so only live
  `SetFlag`s and unfolded ops are left.
- **`insn!(addr, "...")`** marks each guest instruction. `quote!` can't carry
  comments and `prettyplease` would drop them, so the generated crate defines
  `macro_rules! insn { ($($t:tt)*) => {} }`: it survives formatting, reads
  like a comment, and costs nothing. The string is the decoded `Instr`'s
  `Debug` (there's no disassembler yet). A debug feature can later make it
  record the last guest pc for crash reports.

### IR to Rust

| IR | Rust |
|---|---|
| `Const`, `Bit` | literals |
| `Bin` | `wrapping_add` / `wrapping_sub` / `&` / `\|` / `^` / `wrapping_mul` |
| `Not`, `BNot`, `BBin` | `!`, `!`, `&` / `\|` / `^` on `bool` |
| `Adc`, `AdcCarry`, `AdcOverflow` | `prim::add_with_carry(a, b, c).0` / `.1` / `.2`, or one call bound to a tuple |
| `Shift`, `ShiftCarry`, `Rrx` | `prim::shift(..).0` / `.1` (or a tuple), `prim::rrx(..).0`; by a constant, `<<`, `>>`, `rotate_right`, `(v >> i) & 1 != 0`; `Ror` by a register, `rotate_right` |
| `MulLongLo/Hi`, `MulCarry`, `Sext` | the `prim` function of the same name (`mul_long` may be a tuple) |
| `Select`, `BSelect` | `if c { a } else { b }` |
| `BitToWord`, `TestBit`, `IsZero` | `b as u32`, `(v >> n) & 1 != 0`, `v == 0` |
| `GetReg`, `SetReg` | `m.cpu.r[i]` |
| `GetUserReg`, `SetUserReg` | `m.cpu.user_reg(..)`, `m.cpu.set_user_reg(..)` |
| `GetFlag`, `SetFlag` | `m.cpu.flag(..)`, `m.cpu.set_flag(..)`; a run of `SetFlag`s, one write of `m.cpu.cpsr`'s flag bits |
| `GetCpsr`, `GetSpsr` | `m.cpu.cpsr`, `m.cpu.spsr()` |
| `SetCpsr`, `SetSpsr` | `m.cpu.msr_cpsr(v, mask)`, `m.cpu.msr_spsr(v, mask)` |
| `Load`, `Store` | the bus, with the address aligned (`& !(w - 1)`) as `ir::eval` does |
| `Exit` | see below |
| `Insn` | `insn!(..)` |

`prim` is `armv4t::sem::prim`, the functions the interpreter calls, so the
two can't disagree on a carry or a shift (sem-design.md). The CPU methods
are `armv4t::interp::Cpu`'s, for the same reason. The exceptions are
small and checked by `just sst-gen`: a shift by a constant is printed as
the operator `prim::shift` reduces to for that amount, and a run of flag
writes sets the bits `Flag::cpsr_bit` gives, as `Cpu::set_flag` does.

### Printing a block

Most of rustc's cost on a part crate before LLVM (its peak memory) grows
with statements and locals, and LLVM deletes most of them anyway, so the
printer (`codegen::print`) binds as few as it can without changing what
runs:

- `Const` and `Bit` are literals at their uses. A literal gets a `_u32`
  suffix only where nothing else types it (a method's receiver, a cast,
  `==`).
- A value used once is printed inside its use, if it is pure; a read
  (`Get*`) if nothing between it and the statement it ends up in writes
  what it reads (`msr cpsr` and exits count as writing everything, and
  nothing moves past an `rt::stall`, a call on the whole machine); a load if
  no other bus access is between and the use runs unconditionally (not
  under a guard, not in one arm of a `select`), so it still runs exactly
  once, in order. Reads and loads are never moved into an exit, and a load
  never into another load or store (both borrow `m.bus` mutably).
- Results of one multi-result prim that are both used (a result and its
  carry, an `add_with_carry`'s three) come from one call bound to a tuple,
  `let (v3, _, v5) = prim::add_with_carry(..);`, at the first one's place
  (if all their operands are defined by then). The IR keeps them separate
  so that a dead flag is just an unused value (sem-design.md); the printer
  joins what is still live.
- Consecutive `SetFlag`s under one guard, of different flags, with only
  pure values between, are one `m.cpu.cpsr = m.cpu.cpsr & !mask | ...;`
  at the last of them. The values move after what is between, so a read
  between ends the run.

On Emerald this took the generated code from 9.4 statements and 4.9 `let`s
per guest instruction to 4.5 and 1.2. What is left is mostly the `insn!`
markers and register writes. The functions use only
`core`, `armv4t` and `runtime`; the crate names `desktop` only in the
platform alias and `main`.

### Exits

Every exit, side or final, does three things: set `pc` to the next key,
tick, and either `continue` (the `match` takes it from there) or return.
A dynamic exit whose target can't be a block of this function (a return,
an unresolved jump, an exception return) always returns: under recursion a
return lands on the same function's return site, and continuing there
would run the outer call's code on the inner call's host frame, growing
the host stack with every call rather than with guest depth. A `swi`
returns when the machine must yield (a wait halted it, or the call isn't
implemented).

| Exit | Lowering |
|---|---|
| `Fallthrough(a)` | `pc = key(a)` (a block ends after an `msr` to the control byte, and its fallthrough is `rt::rekey`) |
| `Jump(t)`, `t` folds to a constant | `pc = key(t)` |
| `Jump`, `JumpExchange`, `ExceptionReturn`, dynamic | `pc = exit(m, at, thumb, e)` |
| `Swi { comment, next }` | `pc = rt::swi(m, at, thumb, comment)`: `cpu.pc = next`, `m.swi(comment)`, the key |
| `Undefined` | `pc = exit(m, at, thumb, Undefined)` |
| a call to a known function | `let Flow::Jump(a) = f_x(m, KEY); pc = a`, then return if `rt::yielding(m)` |

`exit` is a small runtime helper: `m.cpu.apply_exit(at, thumb, Some(e))`,
then the key from `cpu.pc` and the T bit. `apply_exit` is the interpreter's
own exit code, so target alignment, the T bit, the SPSR copy on an exception
return and exception entry are the same code in both. "Folds to a constant"
means codegen evaluates `Const` and `Bin` of constants in the IR to find the
target (a Thumb `bl`'s target is `lr + offset`, both constants after the
builder's forwarding); it only chooses the static form, and the computed
value is still emitted.

**Jump tables need no special lowering.** A resolved table's targets are
blocks of the function (the walk below adds them), so the dynamic jump sets
`pc` and the loop's `match` already has an arm per target plus the `_`
fallback to the dispatcher. That is decision 8's "every resolved table keeps
a fallback arm", for free. Unresolved and rejected jumps lower the same way
and simply always leave through `_`.

## Calls and guarded returns

A block exit is a **call** when the block wrote lr to the exit's
fallthrough address (in key form) and the exit isn't one of discovery's
far-jump `bl`s (`rec.far_jumps`, decision 5a). That covers `bl`,
`mov lr, pc; bx rN` and GCC's `bl` to a `bx rN` stub.

- **Call to a known function** (the target folds to a function entry): a
  Rust call, then `pc = a; continue`. If the callee returned to the
  expected return site, the `match` continues there. If not, `_` passes the
  address up. That is the guarded return: no comparison is emitted, the
  `match` is the guard. A mismatch that happens to land on another block of
  the caller is also handled locally, which is correct by the invariant.
  `tick` reports a yield once, so after the call the caller checks
  `rt::yielding(m)` (frame ended, halted, fault, IRQ takeable) and returns
  if the callee's tick asked for one.
- **Call through a `bx rN` stub** (a `bl` to a lone `bx rN`, which
  discovery treats as an indirect call, not a function): `Plan::CallVia`.
  The stub's `bx` goes through `rt::exit`, then the crate's `call(m, pc)`
  (`dispatch::call` with its own table) runs whatever block rN names and
  the caller continues at what it returns, normally the return site. A
  key with no block (or no valid overlay) comes straight back, and the
  `match` passes it up to the dispatcher. This is how ROM code reaches
  agb's IWRAM `memcpy` and mixer (#54).
- **Call to anything else** with a constant target that isn't a function:
  the return site still becomes a block, so the caller resumes compiled
  after it.
- **Indirect call** (dynamic target): v1 unwinds, `pc = target` and out
  through `_`. The return site is in the dispatch table, so the caller is
  re-entered there when the callee returns. Since `structured/call-trampoline`
  the call site settles it instead (`rt::settle`: the dispatcher nested
  there until the guest is back at the return site with the caller's sp;
  `docs/structured-codegen-design.md`, step 5).
- **Tail calls** (`b other_fn`, a jump without the lr write): unwind, and the
  dispatcher calls the target. This is what bounds host stack growth from
  guest tail-call chains (decision 8, risks.md). A jump to the function's own
  entry stays local. The caller's call site (above) settles the key it gets
  back, so a chain of tail calls runs in one nested dispatcher, not deeper.

Guest recursion is host recursion, so deep guest call chains need a big host
stack: the desktop runs the dispatcher on a thread with a 256 MB stack.
The deepest host stack hasn't been measured yet (risks.md asks for it in a
long play session); none of the test ROMs come near the limit.

## Which blocks a function has

Codegen builds its own block sets from discovery's output
(`analysis::discovery::Discovery`); it needs nothing new from discovery.

1. Roots: every `disc.funcs` entry (confirmed and scanned) and every
   `disc.entries` root becomes a function.
2. From each root, build superblocks with `ir::build::Builder`, one
   instruction at a time, decoding through `Program::decode` and only at
   addresses `disc.code` has in that instruction set. Both halves of a
   Thumb `bl` go through the builder as two instructions, as the semantics
   expect.
3. Block starts are the root, every static exit target, every exit's
   fallthrough or continuation (including after a guarded `swi` and after a
   guarded call), return sites of calls, and the targets of resolved jumps at
   the block's exits (`disc.rec.jumps`). A block ends at an unguarded exit,
   before the next known block start, after 256 instructions (to bound IRQ
   latency), or where `disc.code` stops (a `Fallthrough` the `_` arm sends
   to the interpreter).
4. Follow block starts to a fixpoint. A start found inside an existing block
   splits it (rebuild with `finish` at the new start). Static targets that
   are another function's entry are not followed; they are calls or tail
   calls.

Code shared by two functions is compiled into both. Every block start of
every function goes into the dispatch table. `report.txt` lists any
`disc.code` instruction no block covers; it should be none.

## Timing

Decision 11 says sync at loop back-edges, after I/O stores and at BIOS
waits. In `loop { match }` form every block transfer goes through the loop
head, so v1 ticks **at every block exit** (side and final), a superset of
back-edges and `swi`. The tick takes the static cost of the
instructions from the block start through the exiting one, summed at
recompile time from the same per-instruction cost the interpreter uses
(`armv4t::cycles::cost`): base cycles plus the code fetches' wait states,
looked up at run time for the block's region (`rt::fetch`), since `WAITCNT`
can change. Data accesses' wait states are added by the bus as it is
accessed, so generated code needn't know which region an address hits; in
ROM code each memory instruction ends with `rt::stall`, the prefetch
buffer's overlap (decision 11).

`m.tick(n) -> bool` returns true when the run loop must get control back:
an IRQ can be taken, the CPU halted, or the frame ended. Generated code then
returns `Flow::Jump(pc)`, with `pc` already the next block, and the whole
host stack unwinds. IRQs and halts are only ever handled at the dispatcher's
top, for compiled and interpreted code alike, so there is one IRQ path
(#27's) and an IRQ's return address is always a block start. Since
`structured/nested-irq` an IRQ or halt doesn't unwind: the tick calls
`yield_at(m, pc)` (`rt::yield_at`), which takes it the same way, through
the dispatcher's top run nested there until the handler is back at `pc`
with the same sp and mode, and compiled code goes on. Only the frame end,
a fault or the depth cap unwind (`docs/structured-codegen-design.md`,
step 6).

**No mid-block sync after I/O stores.** A store's device effect happens in
the store itself (the bus); what a sync adds is IRQ delivery, and an IRQ
taken mid-block would return into the middle of a block, which has no
entry. So an IRQ that an I/O store enables is taken at the end of that block,
at most 256 instructions later, and typically a few. sem-design.md already
allows this ("a few instructions late at most"). If a ROM needs better,
split blocks after stores to constant I/O addresses.

The interpreter ticks per instruction and compiled code per block, so IRQs
land at slightly different instructions in the two. The checks compare
final state (registers at a test ROM's idle loop, frames), not traces.

## `swi`

`Exit::Swi { comment, next }` lowers to `m.cpu.pc = next`, then the same
`swi` entry the interpreter uses (#28), passed the raw comment field. The
HLE leaves `cpu.pc` wherever execution continues: `next` for an ordinary
call, somewhere else for a wait (`Halt`, `IntrWait`, `VBlankIntrWait`), in
which case it also halts the machine. Generated code reads the key back and
ticks, and returns if `rt::yielding(m)`; the dispatcher's top runs time forward
and takes the IRQ, and whatever the HLE does with the BIOS-area addresses is
between #28 and the interpreter fallback. Codegen doesn't model waits.

## The dispatcher and the interpreter fallback

`runtime/src/dispatch.rs`, owned by #31:

```rust
#[must_use]
pub enum Flow {
    /// Continue at this key.
    Jump(u32),
}
pub type GuestFn<P> = fn(&mut Machine<P>, u32) -> Flow;
pub struct Code<P: Platform + 'static> {
    /// Every block start of every recompiled ROM function, sorted by key.
    pub blocks: &'static [(u32, GuestFn<P>)],
    /// The same for the overlays, each with its overlay's index.
    pub ram_blocks: &'static [(u32, GuestFn<P>, u16)],
    /// The overlays' guards: the IWRAM bytes each was compiled from.
    pub overlays: &'static [Overlay],
    pub rom_hash: u64,
    /// Where the desktop writes the fallback's ROM entries (seeds).
    pub seeds_file: Option<&'static str>,
}
pub fn run(m, code: Option<&Code<P>>, cycles, stop_at, stats) -> Stop;
```

**`Machine<P>` stays generic over the platform** (#26, settled with the
overseer). `GuestFn`, `Code` and the dispatcher are generic over `P` in the
runtime; the generated crate makes them concrete with one line,
`pub type P = desktop::Desktop;`, and its prelude's
`pub type M = runtime::Machine<crate::P>;` is what every generated fn takes.
An ESP32 build changes the alias (and the `main`).

`Flow` has one variant. It stays an enum because it is the type every
generated fn returns, and a later variant (a stop request, ABI-trust write
back) shouldn't mean regenerating every signature by hand.

The dispatcher loops until the frame ends:

1. Let #27's code handle halts and take a pending IRQ (`cpu.pc` moves to the
   handler, through the HLE'd BIOS IRQ stub).
2. Look the key up (binary search). Found: call the fn, write the returned
   key back to `cpu.pc`.
3. Not found: the **interpreter fallback**. Step the interpreter (#27's
   per-instruction step, which ticks) until the key is in the table, the
   frame ends, or an IRQ or halt needs step 1. v1 checks the table after
   every instruction (simple, and the fallback is the slow path anyway); a
   block-start bitmap is the fix if that shows in a profile.

Thumb or ARM comes from the key's bit 0, which is the T bit, as on
hardware.

**Overlays** (decision 7, #54). Code the ROM copies into IWRAM is
recompiled at its IWRAM addresses, one overlay per block (numbered from
`Func::overlay` in address order), and its block starts go in a second
table, `Code::ram_blocks`, each with its overlay's index. A RAM key is
found only while the overlay is valid (`runtime/src/overlay.rs`): IWRAM
still holds its instruction bytes. Each block's match arm checks
`rt::overlay_valid` before it runs, since a store in one block may rewrite
the next, and ticks through `rt::tick_ram`, which counts its cycles as RAM
code's. A block the code patches may have more versions
(`codegen/src/variants.rs`, from a recompile-time interpreter run), each an
overlay guarded by its own bytes; its arm runs whichever version is valid.
The same run finds copies discovery missed (`codegen/src/seen.rs`): IWRAM
code that runs outside every known copy, below `sp`, whose bytes are in
the ROM.
`report.txt` lists each copy and each function's overlays (IWRAM address,
ROM source, bytes, blocks) and its blocks' patched versions. A RAM key with no valid
overlay (code built in RAM, code on the stack, rewritten code)
falls back as before.

**Stats.** `Stats` counts dispatcher calls into compiled code (with how
many resumed a function past its entry, `Code::is_entry`, and how many
came back yielding, for `docs/structured-codegen-design.md`) and
instructions the fallback interpreted, and the distinct ROM keys the
fallback was entered at, in a fixed array (no alloc in `runtime`). The
desktop prints them on exit (decision 1's "would change our minds"; #32
can turn them into cycles once #27's clock is in). The ROM keys are
decision 6's seeds; the desktop writes them to `out/<name>/seeds.txt`
(`Code::seeds_file`), when that folder is there (not from a binary
copied elsewhere, like a Steam Deck build). Feeding them back into discovery needs a
`discover` that takes extra roots, which is a follow-up for the discovery
thread's owner, not this sprint.

## `just recompile` and running it

```text
just recompile ROM [OUT] [--name NAME]  # default OUT = out/<name>, the game's name
just run-recompiled OUT [desktop args]
just recompile-check ROM [FRAMES]  # recompiled vs --interp: frame hash and registers
just sst-gen [PER_FILE]            # SingleStepTests through generated Rust
```

The recipes take the build lock (`CARGO_BUILD_JOBS=6 flock
/tmp/gbaa-build.lock`) around each cargo step themselves, so running a built
binary never waits; callers never wrap `just` in it, which would deadlock.
Generated binaries run under
`systemd-run --user --scope -p MemoryMax=3G -p MemorySwapMax=0`, so a
runaway can't take the desktop down.

`recompile` runs `gbaa-recompile` (a bin in `crates/codegen`): discovery,
blocks, lowering, then writes:

```text
out/<name>/
  Cargo.toml       # its own [workspace]; path deps on runtime, armv4t, desktop, base, parts
  base/src/lib.rs  # insn! macro; prelude; P alias; CODE declared by symbol
  parts/pNN/src/lib.rs   # the other parts' functions it calls, by symbol
  parts/pNN/src/mNNN.rs  # 256 functions per module, for rustc's codegen units
  src/lib.rs       # pub static CODE, exported as `gbaa_code`
  src/table.rs     # the sorted (key, fn) array
  src/main.rs      # desktop::main(Some(&CODE))
  src/bin/call-<name>.rs  # call one guest function, recompiled or not
  report.txt       # functions, blocks, uncovered code, unresolved jumps
  seeds.txt        # written by a run: where the fallback was entered
```

- Generated files go through `prettyplease`; one `syn::File` per module.
- **The code is split into part crates** of at most `PART_INSNS` (20,000)
  guest instructions, so no one rustc holds a whole big ROM: Emerald's
  815k instructions in one crate took over 16 GB; as 42 parts (450 MB of
  Rust) it builds in about 7 min of wall time with no process over 2.3 GB,
  under an 8 GB cap. Parts can't depend on each other (calls go both ways),
  so a function called from another part is exported by symbol
  (`gbaa_f_XXXXXXXX`) and declared there in an `extern "Rust"` block, behind
  a safe wrapper; `CODE` reaches the parts the same way (`gbaa_code`).
- **A generated `main`, not `gbaa-desktop --recompiled`.** The desktop
  binary can't load a Rust crate at run time without a dylib ABI; the
  generated binary links `desktop` as a library and has exactly its CLI
  (`ROM`, `--headless`, `--frames`, `--dump`, `--hash`, `--regs`), plus `--interp` to ignore
  the table. So one binary runs both sides of the interpreter-vs-recompiled
  check.
- The ROM is not embedded: the binary takes its path, as `gbaa-desktop`
  does, and refuses a ROM whose hash doesn't match `CODE.rom_hash` (FNV-1a
  64, no dependency).
- `run-recompiled` builds with `--target-dir target/recompiled`, shared
  between games so `runtime`, `armv4t` and the desktop deps build once, in
  release. The generated `[workspace]` keeps cargo from treating it as a
  stray member of ours; it inherits `rust-toolchain.toml` from the repo
  root.
- Building a generated crate is heavy work (brief #36): one at a time, and
  the recipe checks `free -g` available ≥ need + 3 GB before building. The
  need is 4 GB (`GBAA_GEN_NEED_GB`). Measured release builds of the
  generated crates, with the deps already built: the LLVM sample 9.6 s and
  0.4 GB peak, jsmolka's ROMs about 2 s and 0.2 GB, june's first-frame ROM
  (138 functions, 16,700 instructions) 100 s and 1.6 GB, the 22,500-fn SST
  crate 14 s and 1.7 GB.

## How the code phase checks itself

In order, cheapest first:

1. **Unit tests** in `cargo test -p codegen` (in `just check`): block walks
   and lowering on hand-assembled ARM and Thumb programs with hand-written
   discovery output: calls and return sites, a guarded call splitting a
   block, Thumb `bl` pairs, a table recorded under a `bl` pair's first
   half, entries without code, a misaligned constant target, `msr`
   rekeying, returns that leave the function, crate names. They check the
   plans and fragments of the printed code, not whole snapshots.
2. **`just sst-gen`** (needs `just fetch-tests`): a generated crate with one
   fn per SingleStepTests case (a sample per file, a few hundred each), each
   one block, run against the expected after-state like `just sst-ir`. This
   tests every IR-to-Rust rule against the same oracle the interpreter
   passes, before any ROM.
3. **ROM differential**, the issue's check: the LLVM sample and jsmolka's
   `arm.gba` and `thumb.gba`, recompiled, run headless with and without
   `--interp` to the idle loop, final registers equal, and `arm`/`thumb`
   report pass (`r12 = 0`).

If the ROM check disagrees and the cause isn't obvious, the debugging aid is
block lockstep: clone the `Machine`, run one compiled block, run the
interpreter on the clone to the same key, compare. That needs `Machine:
Clone`, asked for below.

## Requirements on other threads

For **#26 (runtime core, merged)**: met, with `Machine<P>` generic (handled
by the platform alias above).

- `m.cpu` is a public `armv4t::interp::Cpu`, the interpreter's own type.
- Memory is reachable from `&mut Machine` with `interp::Bus`'s contract
  (aligned address, zero-extended loads, low bytes stored), as a field
  disjoint from `cpu` (the interpreter's `step` needs both at once). The
  accessors inline EWRAM and IWRAM, so a constant `Width` folds away in
  generated code (decision 2), and reach the rest of the map through one
  out-of-line call (inlining all of it doubled rustc's time on a part).
- `Machine: Clone` (without the platform), for the lockstep aid. Nice to
  have, not blocking.

For **#27 (interpreter, timing, IRQs)**:

- `tick(&mut self, cycles: u32) -> bool`: advance time; true when the run
  loop must take control (IRQ takeable, halted, frame ended). Inlined fast
  path: an add and a compare.
- A per-instruction base cycle estimate as a pure function of the decoded
  `Instr`, callable at recompile time, which the interpreter also uses.
  Wait states, if modelled, added by the bus on access.
- The halt / IRQ-entry step and the one-instruction interpreter step as
  functions the dispatcher can call, plus a monotonic `u64` cycle counter
  and a frame-ended flag.
- Anything that reads `cpu.pc` from outside (the gba-tests idle-loop
  check) does it between frames, where it is exact for compiled code too.

For **#28 (BIOS HLE)**: one `swi` entry taking the raw comment field (ARM's
24-bit or Thumb's 8-bit, decoded inside using the T bit), called with
`cpu.pc` = the next instruction, leaving `cpu.pc` where execution continues.
The interpreter calls the same entry.

For **#29 (desktop)**: `crates/desktop` is a library plus the
`gbaa-desktop` bin, with `desktop::main(code: Option<&'static Code>)`
parsing the CLI, so a generated `main` is one line. The run loop goes on a
thread with a large stack.

Known deviation: open-bus reads depend on the prefetched instruction, so
they need `cpu.pc`, which compiled code doesn't keep current. The
interpreter stays the reference for `memory.gba`; if a game relies on open
bus, generated code can store `cpu.pc` before loads from unknown addresses.

## On #27's clock and #28's BIOS

The dispatcher calls what #27 and #28 merged: `Machine::tick` (generated
code's `rt::tick`), `take_irq`, `idle`, `step` (the fallback, which also
runs the HLE'd BIOS code such as `IRQ_RETURN`), `sched.frame_ended`,
`Machine::fault`, `Machine::swi`, and `armv4t::cycles::cost` for each
block's cycles. `dispatch::run_frame(m, code, stats)` has the same shape as
`Machine::run_frame` (halt, then IRQ, then run), and the desktop calls one
or the other. (Before they merged, codegen was built against stand-ins in
`dispatch.rs`: a fuel counter for `tick`, the hardware exception for
`swi`, one cycle an instruction.)

## What's been checked

All after merging #27 and #28:

- `just check`, including `cargo test -p codegen` (above) and the
  dispatcher's tests in `runtime`.
- `just sst-gen 500`: 22,500 SingleStepTests cases (500 from each of the 45
  files), each as its own generated fn: 0 differ from the interpreter, peak
  RSS 66 MB. It found one bug: a constant ARM jump target with its low bits
  set (a writeback to pc) was aligned by the static key; such exits now go
  through `rt::exit`.
- The LLVM sample: it only spins at its entry, so instead of "run to the
  same final registers" its two jump-table functions (Thumb and ARM) are
  called through `call-llvm` with every selector (0-11 and 99): registers
  equal to the interpreter's in all 26. The Thumb one runs entirely in
  compiled code (one dispatcher entry); the ARM one's interworking calls to
  its Thumb leaves are indirect, so they unwind once each (two entries, 0
  instructions interpreted).
- jsmolka's `arm`, `thumb`, `memory`, `nes` and `bios` (`just
  recompile-check`, 60 frames): all pass (`r12 = 0`), last frame hash and
  registers equal to `--interp`, every discovered instruction in a block.
  `nes` runs from code it copies to RAM, so it stays in the fallback.
- june's first-frame ROM (`$GBAA_DATA/agb/gba-cart1.gba`, information for
  #32): the picture draws, frame hash and registers equal to `--interp` at
  60 frames; 564 dispatcher entries and 6,570 interpreted instructions over
  the 60 frames (5 ROM addresses, plus agb's IWRAM code).
- An independent review found six real bugs, all fixed: a GCC
  case-helper table under a `bl` pair's first half, entries without code
  that spun, recursion running outer calls on inner frames, a callee's
  yield getting lost, `msr` changing T under a static key, and ROM file
  names that aren't crate names. Checking june's ROM found a seventh: a
  `bl` to a `bx rN` stub dropped its return site. The unit tests cover the
  plans for each and the printed `return` after a return-like exit, the
  `rt::yielding` checks after a call and a `swi`, and the rekey; recursion
  unwinding at run time is covered only by the lowering, not by a run.

`recompile-check` doesn't fail on a high fallback share; the recompiled
run prints the counts to the terminal (stderr) for a person to read.

## Defaults taken (accepted by the overseer)

- Tick at every block exit; no mid-block I/O sync; 256-instruction block
  cap.
- A constant ARM jump target with low bits set goes through `rt::exit`
  (the interpreter keeps the bits in `cpu.pc`).
- IRQs, halts and frame ends unwind the whole host stack rather than run a
  nested dispatcher inside `tick` (IRQs and halts run one since
  step 6 of `docs/structured-codegen-design.md`).
- Indirect calls and tail calls unwind in v1 (settled at the call site since
  step 5 of `docs/structured-codegen-design.md`).
- The dispatcher lives in a new `runtime/src/dispatch.rs`, owned by #31.
- A generated `main` linking `desktop` as a library.
- Fallback seeds go to the gitignored `out/<name>/seeds.txt`. Decision 6
  says seeds are committed per game, but the brief bars committing symbol
  maps and test data; a seed file is close to a symbol map, so it stays out
  of git until june or the overseer says otherwise.
- Scanned functions (decision 5d) are compiled like confirmed ones: a false
  one costs code size, never correctness, since it only runs if execution
  reaches it.
