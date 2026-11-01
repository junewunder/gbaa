# Registers as locals, calling conventions and types

This is one of the four literature reviews behind the structured codegen
design. It is unit `structured/lit-registers`, for `structured/design`, and
part of phase 2 of the plan of 2026-10-21 in
[open-decisions.md](../open-decisions.md). The question: can guest
registers, flags and stack slots move out of `Machine` into Rust locals,
and can we recover functions' signatures and types, while the frame hash
still matches the interpreter? What would each technique cost, and what
would it buy in run time or compile time?

Researched 2026-10-24 on quack. Sources were read in their primary form:
the papers, and the tools' source trees at the commits under "Sources"
(shallow or sparse clones, never built or run). Hex-Rays is closed, so its
claims come from Hex-Rays' own documentation and talks. Every source is
also in [references.md](../references.md). The companion reviews are
[structured-control-flow.md](structured-control-flow.md) (structuring) and
[static-recompilers.md](static-recompilers.md) (what other recompilers do
about flags, interrupts and timing); this one cites them where they meet.

## Summary

- **Registers can move into Rust locals exactly, with no ABI assumption.**
  Every place where something outside a function reads or writes the CPU
  is visible to codegen. There are about a dozen kinds ("Where state must
  be in `Machine`"). Write back at each, reload after the ones that write,
  and the dispatcher, the interpreter, an IRQ and every callee see the
  interpreter's state.
  - This is QEMU's sync-and-kill discipline for TCG globals, Dolphin's
    register cache flushed "in far code" on exits, and gpsp's store and
    reload around C calls.
  - We get it cheaper than they do: our bus accesses can't fault and don't
    read the CPU, and a tick reads nothing on its fast path. So the
    write-backs sit only on the cold yield path, at calls, at exits and at
    the 25 mode-switching instructions in all of Emerald.
- **LLVM does the promotion for us.** A spike in our shape
  (`let mut r = m.cpu.r;`, `m.cpu.r = r;` before each return) compiled with
  rustc 1.99.1: SROA split the array and kept the used registers in host
  registers across opaque `tick` and I/O calls and the loop's back edge,
  with no `cpu.r` traffic in the loop. LLVM's mem2reg is Cytron's
  algorithm, and its rules are simple: constant indices only, no address
  taken. One dynamic index turns SROA off for the whole array
  (`SROA.cpp:1168-1169`). Braun et al.'s on-the-fly SSA is what our IR
  builder already does inside a block. We'd need it across blocks only for
  analyses (cross-block const-prop, the preservation proofs), not for
  promotion.
- **No register write may be dropped across a tick, even a dead one.** The
  BIOS IRQ entry pushes r0–r3, r12 and lr to the IRQ stack, and C handlers
  push r4–r11, so any register's value at any tick can reach memory. With
  locals that costs nothing: the value stays in its local, and its only
  use is the cold write-back, which LLVM can sink there. If the flags are
  locals too, the yield path writes the exact CPSR. That closes the
  `IntrMain` gap the recompilers review found, with no extra code per edge.
- **Something exact can ship now, before any locals:** 307,395 of
  Emerald's 687,497 register writes (45%) are overwritten later in the same
  block with nothing in between that can read `cpu.r`. Dropping them is a
  builder change in safe mode. LLVM already deletes some of them where the
  bus's fast path inlines (seen in the Emerald binary), so the gain is
  mostly rustc's front end: about 8% of the statements.
- **Decision 10's summaries buy much less than it assumed.** With
  registers in locals, a summary only saves the write-back of registers a
  callee never touches and the reload of registers it provably preserves:
  about a dozen host loads and stores per call, at 53,500 call sites. It
  can't do what XenonRecomp and Anvill do, which is to delete the callee's
  saves, because the stack is guest memory. Safe mode with locals gets most
  of the win, so summaries should wait for a measurement that says the
  call write-backs matter.
- **If summaries are built, Van Emmerik's thesis is the design and rev.ng
  is the check.**
  - Van Emmerik (2007): bottom-up over call-graph SCCs; unknown callees
    read and write everything (`<all>`); recursion by premises; prove
    `x0 = x_exit`.
  - rev.ng: fold `reg_exit − reg_entry` to 0.
  - Both trust a restore from a stack slot without proving that nothing
    wrote the slot in between. On the GBA that needs a value-set proof
    that no store between the push and the pop can hit the frame, or the
    function stays in safe mode.
  - Ghidra, RetDec and Hex-Rays check nothing at calls. They assume the
    convention ("Assumes … AAPCS Applies"; Guilfanov: "configurable
    assumptions"). The spec for open edges is agbcc's: r4–r10 and sp
    preserved; r0–r3, r12, lr and the flags clobbered; sp aligned to 4.
    Neither Ghidra's ARMv4T spec nor rev.ng's AAPCS.yml matches it.
- **Stack slots stay in memory.** Pushes, pops and sp-relative accesses are
  guest memory, visible to IRQ handlers, DMA and pointers, with wait
  states. Every decompiler that promotes them assumes no aliasing
  (Hex-Rays: "one of the assumptions our decompiler makes"). There is
  little to win anyway: sp-relative loads and stores are 1.6% of
  Emerald's instructions. agbcc keeps locals in registers, and its stack
  traffic is saves and restores that must happen.
- **Type recovery buys nothing exact.**
  - TIE, Retypd and BinSub are sound relative to their constraints or to
    source types (90–98% "conservative"), not to behaviour; OSPREY and
    DIRTY are probabilistic.
  - Ghidra's and Hex-Rays' types are presentation, with one exception:
    Ghidra splits loads and stores by struct type, which is wrong for GBA
    I/O, VRAM and wait states.
  - What does buy speed is a *region* fact (Lasagne's "stack address, no
    fence"), and our bus already takes the guarded version: EWRAM and
    IWRAM inline on `addr >> 24`. A proved region saves one compare.
- **What nobody has measured: what this does to our compile time.**
  Locals replace memory traffic, which LLVM's MemorySSA, GVN and DSE work
  hard on, with φs at v1's loop head, one per held register and block
  exit. Instrew's register convention cost 12% more compile time for 14%
  less run time. The design's first spike should generate one part crate
  three ways and measure.

## What we start from

The code is described in [codegen-design.md](../codegen-design.md). For
this review, what matters is its one invariant: **all guest state lives in
`Machine` at every block boundary.** Registers are `m.cpu.r[i]`, a
`[u32; 15]` of the current mode's r0–r14 (`armv4t::interp::Cpu`, with the
banked r8–r14 in `cpu.banks`). Flags are bits of `m.cpu.cpsr`. A generated
frame holds only `pc`. Within a block the IR builder forwards register
values (a read after a write reuses the written value), but "every write
is still emitted as an effect, in order" (`analysis/src/ir/build.rs:14-17`).
So every block starts by loading what it reads from `m.cpu.r` and every
register write is a store.

Decision 10 planned the next step: summaries of what each function reads,
clobbers and preserves, inferred where every caller is a known `bl` and
checked against AAPCS at open edges, with failures kept in "safe mode".
`docs/sem-design.md` ("Sync points inside a block") expects a `Sync` inst
where locals must be written back.

What a generated function looks like today, the first block of Emerald's
`CreateTask` (`f_080a8fb1`, in `out/pokemon-emerald-version/parts/p14`),
trimmed:

```rust
let v2: u32 = m.cpu.r[13];
let v4: u32 = v2.wrapping_add(0xffff_ffec);
m.bus.store(Width::Word, v4 & !3, m.cpu.r[4]);   // push {r4-r7, lr}
// ...
m.cpu.r[13] = v2.wrapping_add(0xffff_ffec);
m.cpu.r[2] = prim::add_with_carry(m.cpu.r[0], 0, false).0;
let v33: u32 = m.cpu.r[1] << 24;
m.cpu.r[1] = v33;                                 // lsl r1, r1, #24: dead
m.cpu.r[1] = v33 >> 24;                           // lsr r1, r1, #24
m.cpu.set_flag(Flag::C, (v33 >> 23) & 1 != 0);
m.cpu.r[6] = 0;
// ...
pc = 0x080a_8fbd;
if rt::tick(m, cycles) {
    return Flow::Jump(pc);
}
continue;
```

### What "exact" asks of registers in locals

The unit's concerns, as they apply to this topic:

| Concern | Exact if |
|---|---|
| Every flag a later instruction may read | Flags held in locals are composed into the CPSR wherever anything outside the function can read it, including an IRQ's saved SPSR (static-recompilers.md, the `IntrMain` gap). |
| Every register value, even a dead one | At every point where control can leave compiled code (a tick's yield, a call, an exit), `m.cpu.r` holds the interpreter's values for all fifteen registers. A dead value is not optional: the BIOS IRQ entry pushes r0–r3, r12 and lr to the IRQ stack (`runtime/src/sched.rs:250-257`), and C handlers push r4–r11, so every register's value at every tick can reach memory. |
| Cycle counts and tick points | Moving state into locals changes no tick: the same `rt::tick` at the same edge. A spill or reload costs host time, never guest cycles. |
| I/O order | No bus access is added, removed or moved. Stack slots are guest memory: a push, a pop and a stack load still go to the bus, in order, with their wait states. |
| IRQs | An IRQ is taken at the same tick with the same `Machine`, so the spill must be complete before the dispatcher (or a nested handler) runs. |
| Self-patching RAM code | An overlay whose guard fails leaves through the exit path, which spills. |
| The interpreter fallback | The interpreter only runs after a `Flow` return, so after a spill; compiled code is only entered through a function, which loads. |

## Measured

From `out/pokemon-emerald-version` (generated 2026-10-22, the same tree
as the companion reviews), counted by the one-off script in
"Reproducing":

| | Emerald |
|---|---|
| Functions, blocks (module files) | 15,167, 112,940 |
| Register writes `m.cpu.r[i] = ` | 687,497 (6.1 per block) |
| Register reads `m.cpu.r[i]` | 196,705 (1.7 per block), 196,185 of them before the block writes that register |
| Writes overwritten later in the same block, with nothing between that could read `m.cpu` | 307,395 (45% of all writes) |
| Of those, r0 / r1 / r14 / r13 | 140,695 / 54,485 / 54,552 / 19,101 |
| Direct calls to generated functions | 53,500 sites |
| Functions with no direct caller (reached only through the dispatcher, a pointer or a table) | 7,031 (46%) |
| Functions with exactly one calling function | 4,544 |
| Stack instructions: push/pop (`Block` on sp), `str`/`ldr` sp-relative | 41,965; 8,919 / 4,053 |
| Literal-pool loads (`ldr rN, [pc, #k]`) | 57,657 (their values already folded, decision 9) |
| Mode switches in compiled code (`msr cpsr`, `msr spsr`, user-bank ops, `mrs spsr`) | 5, 1, 17, 2 |

And from pokeemerald's own link map (`pokeemerald.map`), where Emerald's
code comes from: 1,975,172 bytes of `.text` from agbcc-compiled C,
5,372 from hand-written assembly (`m4a_1.s`, `crt0.s`,
`libgcnmultiboot.s`, the ROM header) and 9,400 from libgcc and libc.
Two C files have `NAKED` functions (`librfu_intr.c`, `multiboot.c`). So
99% of Emerald's code is compiler output, and the calling convention is a
good bet for nearly all of it, but not for the sound mixer and the IRQ
entry, which are exactly the hot code.

What the binary does with this already: in the Emerald build of
2026-10-22 (`target/recompiled/release/gbaa-pokemon-emerald-version` in
the main checkout), `objdump -d` of `CreateTask` shows LLVM deleting some
of the dead in-block stores itself (the bus's EWRAM fast path is inlined,
so it can see nothing reads `cpu.r` between). It cannot keep a register in
a host register across a block boundary: every block re-loads from
`0xa148c(%rbx)` onward (`cpu.r`) after the loop head's indirect jump,
because the slow paths of `tick` and of the bus are calls that may read
`Machine`.

### A spike: one local array, written back on the way out

To see what LLVM does with the obvious lowering, a 70-line model of our
shape (its source is in "Reproducing"): a
`Machine` with `cpu.r: [u32; 15]`, a `tick` whose slow path is an
`#[inline(never)]` call taking `&mut M`, an opaque I/O store, and a
two-block `loop { match pc }`. Once as today, once with
`let mut r = m.cpu.r;` at entry and `m.cpu.r = r;` before each `return`.
`rustc 1.99.1 -O --emit=asm` (x86-64):

- **Today:** every block loads its inputs from `(%rbx)`, `4(%rbx)`, …,
  stores every result, and `r3 -= 1` is `decl 12(%rbx)`, a
  read-modify-write in memory, as in the Emerald binary.
- **Locals:** SROA split the array; the four registers the code uses live
  in host registers (`%edx`, `%r15d`, `%r12d`, `%ecx`) across both opaque
  calls and the loop's back edge. LLVM merged the three write-backs into
  one exit block. There is no `cpu.r` traffic inside the loop at all.
- **One waste:** the eleven registers the function never touches were
  copied to the host stack at entry and back at exit, since LLVM can't
  prove `cpu.r` unchanged across the opaque calls. Codegen knows which
  registers a function reads and writes, so it should hold only those in
  locals and write back only those it writes.

So the promotion itself is LLVM's: we don't need to build SSA to get it.
What codegen must supply is the write-back at every point in the next
section, and nothing that takes a local's address or indexes the array
with a non-constant.

There is no run-time profile of compiled Emerald on quack (`perf` isn't
installed), so the share of time that register traffic costs is not
known. The desktop already runs 70× faster than real time
(sound-design.md), so on the desktop the payoff of this work is compile
time and the ESP32, not frames per second.

## Where state must be in `Machine`

The whole topic turns on one table: each place generated code hands
control to something that reads or writes `Machine`'s CPU state. It comes
from `runtime/src/dispatch.rs`, `runtime/src/sched.rs` and the printer.

| Point | What reads or writes the CPU | With registers in locals |
|---|---|---|
| `rt::tick`, fast path (`sched.rs:92-104`) | the clock, `io.events`, `irq_line`: no CPU state | nothing |
| `rt::tick`, slow path (`Machine::sync`) | scanlines, timers, DMA, audio: memory and I/O, no registers. `irq_takeable` reads the CPSR's I bit, a control bit | nothing, if locals hold only NZCV and control bits stay in `cpu.cpsr` |
| A tick's yield return, and `_` | the dispatcher: `take_irq` reads r13, pushes r0–r3, r12, r14 and the CPSR; save states, rewind and Lua run at the frame end (lua-scripting.md); the interpreter | write back all registers and flags, then return |
| A call to a generated function | the callee reads any register and writes any | write back before; reload after (safe mode). A checked summary shrinks both sets (below) |
| Return from a call with `rt::yielding` | `Machine` holds the callee's state at its yield, exact | return **without** writing back: the caller's locals are stale, and it will be re-entered at its return site through the dispatcher, which reloads |
| `rt::exit` (`apply_exit`) | reads and writes the CPSR (T bit, an exception return's SPSR copy and bank switch) | write back before; it always leaves |
| `rt::swi` (`Machine::swi`, the BIOS HLE) | reads r0–r3, writes r0–r3 (and more for some calls), may halt | write back before, reload after |
| `msr`, `mrs`, `ldm`/`stm` with `^`, `GetSpsr` | the CPSR, SPSRs and banks; a mode change swaps r8–r14 | write back, run, reload: 25 sites in all of Emerald |
| `rt::call` through a `bx rN` stub | as a call | as a call |
| An overlay guard that fails | leaves for the dispatcher | as `_` |
| Function entry, at any key | — | load the registers and flags from `m.cpu` |
| A nested IRQ inside the tick (static-recompilers.md, item 3) | the handler | write back, run it, reload; unwind as now if it returns elsewhere |
| A frame end handed off without unwinding | save states, Lua, rewind | write back first. A save state *loaded* there replaces `Machine`, so it must unwind every compiled frame |
| Bus loads and stores, `rt::stall`, `rt::block`, `rt::skipped`, `rt::multiply`, `rt::fetch` | the bus and the wait-state counters only | nothing |

Three things follow.

1. **Promotion inside a function needs no ABI assumption.** Every
   boundary in the table is local and visible to codegen. Write back at
   each, reload where the table says, and the state the dispatcher, the
   interpreter, an IRQ or a callee sees is the interpreter's, by
   construction. That is "safe mode" with the registers still in locals.
   Decision 10's summaries are only about the second row: which registers
   a direct call must write back and reload.
2. **No register write may be dropped across a tick.** A register value
   that is dead in the dataflow sense (overwritten before it is read) is
   still pushed to the IRQ stack if an IRQ lands at the tick between. With
   locals this needs no analysis: the local holds the value, and its only
   use is the cold write-back on the yield path, which LLVM can sink into
   that path. The same holds for flags: if flags are locals, the yield
   path composes the exact CPSR, and the `IntrMain` gap closes for free.
3. **Writes overwritten inside a block, with nothing in the table between
   them, can be dropped today, in safe mode.** That is 307,395 of Emerald's
   687,497 register writes (45%). It is exact: nothing between the two
   writes can read `cpu.r`. It is the block-local half of promotion, and it
   is a builder change, not a new pass: emit only each register's last
   write before the next boundary.

## Registers in locals: the techniques

### SSA construction: Cytron et al. and Braun et al.

- **Cytron, Ferrante, Rosen, Wegman and Zadeck** (TOPLAS 1991) is the
  eager, classic construction. It computes dominance frontiers, places φs
  at the iterated frontier of each variable's definitions (§4–5.1) and then
  renames in a walk over the dominator tree. It needs the whole CFG and its
  dominator tree first. The result is minimal for any CFG, but not pruned
  unless liveness is added. LLVM's mem2reg is this algorithm
  (`PromoteMemoryToRegister.cpp:9-13`).
- **Braun, Buchwald, Hack, Leißa, Mallon and Zwinkau**, "Simple and
  Efficient Construction of SSA Form" (CC 2013), works backwards and on
  demand, straight from an AST or bytecode.
  - `writeVariable` records the current definition per block.
    `readVariable` is local value numbering, and goes recursive only on a
    miss (Alg. 1–2, pp.3–5).
  - A block is *sealed* once no more predecessors will be added. Until
    then a read places an operandless "incomplete" φ, which sealing
    completes (Alg. 4, p.6). A φ that names only itself and one value is
    replaced by that value, recursively (Alg. 3, p.5).
  - The result is pruned, and minimal for reducible CFGs (§4). On
    irreducible ones it can leave redundant φ cycles, which an SCC pass
    removes (Alg. 5, §3.2).
  - In LLVM 3.1 on SPEC CINT2000 it was as good as mem2reg (0.28% fewer
    executed instructions, Table 2). Its point is skipping the
    alloca/load/store detour, which cost 25% more instructions before
    promotion (Table 1).
- **Exact for us?** Both are exact: SSA renaming changes no semantics. The
  question is only who builds it.
  - Our IR builder *is* Braun's local value numbering already
    (`ir/build.rs:14-17`: "a read after a write in the same block reuses
    the written value").
  - Extending it across blocks is Braun's recursive case. Ramsey's
    structuring visits blocks in dominator order, so sealing is natural.
  - **But we don't need it for promotion.** The spike above shows rustc and
    LLVM do it from plain Rust locals. We would want our own global SSA
    only for analyses that need values across blocks: const-prop's
    "entry values across blocks" (sem-design.md, still open), constant
    addresses for I/O stores, and the preservation proofs below.
- **Cost.** Braun is linear in practice and needs no dominator tree.
  Emitting locals costs codegen nothing, and LLVM's mem2reg/SROA is
  linear-ish, though for a v1-shaped function every local becomes a φ at
  the loop head, one operand per block exit. Whether that costs LLVM more
  than today's memory traffic (MemorySSA, GVN, DSE over 687k stores) is the
  first thing the design's spike should measure on a part crate (p07: 1.13
  GB, 56 s today).

### LLVM's mem2reg and SROA, from Rust locals

`isAllocaPromotable` (`llvm/lib/Transforms/Utils/PromoteMemoryToRegister.cpp:66-114`,
at `bfe0941`) accepts an alloca used only by non-volatile loads and stores
of one type. Any other use, a call taking its address in particular, makes
it unpromotable. SROA (`llvm/lib/Transforms/Scalar/SROA.cpp`) first splits an
aggregate by constant byte offset, including `memcpy`s in and out
(l.1220-1232), then hands the pieces to mem2reg (l.6496-6507).

- `let mut r = m.cpu.r;` is a `memcpy` into a local `[15 x i32]`. With only
  constant indices, SROA splits it into fifteen scalars. The write-backs
  `m.cpu.r = r` become fifteen scalar stores, exactly where we wrote them.
  That is what the spike showed.
- One **dynamic** index anywhere turns it all off. A store at an unknown
  offset aborts SROA for the whole array (l.1168-1169); an unknown load
  leaves only store-to-load forwarding (l.1146-1149). Our IR never indexes
  a register dynamically (`Reg` is static per instruction, and `ldm`/`stm`
  lists are unrolled), so the printer must simply keep it that way, or use
  fifteen named locals.
- LLVM can drop a write-back only where it proves it redundant, never
  across an opaque call that receives `m`. So it can't break exactness.
- It needs `opt-level` ≥ 1; SROA doesn't run at 0. Every build of generated
  code is optimised already.

### QEMU's TCG globals: sync and kill

QEMU (`d731492`) makes each guest register a TCG *global* backed by a
`CPUARMState` field (`target/arm/tcg/translate.c:67`). The register
allocator keeps globals in host registers inside a translation block. The
rules for writing them back are in the liveness pass (`tcg/tcg.c`):

- A helper call is classified by flags (`include/tcg/tcg.h:228-243`).
  - `TCG_CALL_NO_READ_GLOBALS`: nothing. It reads no guest register,
    "either directly or through an exception".
  - `TCG_CALL_NO_WRITE_GLOBALS`: *sync*. Dirty globals are written back,
    and the host copies stay valid (`la_global_sync`, l.3734).
  - Neither flag: *kill*. Write back, then forget, and reload after
    (`la_global_kill`, l.3780).
- Guest loads and stores are `TCG_OPF_CALL_CLOBBER | TCG_OPF_SIDE_EFFECTS`
  (`include/tcg/tcg-opc.h:128-131`), so they sync. A guest access can
  fault, and the fault handler reads the registers.
- The end of a block saves all globals (`tcg_reg_alloc_bb_end`,
  l.4856-4880). `cpsr_write` has no flags (`helper-defs.h:65`), so a mode
  change kills, which suits the banked r13/r14 it swaps in memory.
- The pc isn't written per instruction. `insn_start` records a row per
  guest instruction, and a fault maps the host pc back to it
  (`accel/tcg/translate-all.c:88-100`, `cpu_restore_state_from_tb`).

**For us:** this is the model to copy, with our table above as the
classification. The difference is in our favour: our bus accesses can't
fault and don't read the CPU, so they need no sync, and QEMU's per-access
sync has no counterpart. A tick is "no read" on its fast path and "kill"
only on its yield. QEMU still saves everything at the end of each block;
our blocks end at a `continue` within the same Rust function, so we can
keep registers across them. QEMU can't, since each translation block is a
separate host function. **Exact: yes**, if every runtime entry point is
classified honestly. **Cost:** none given in the source.

### Lifters: Remill, McSema, Anvill

- **Remill** (`56918a8`) keeps all registers in one `State` struct, passed
  by pointer, "so it must be accessed via load and store"
  (`docs/LIFE_OF_AN_INSTRUCTION.md:217`). That is our `Machine`.
- **McSema** (`2636805`) makes `State` a thread-local global
  (`mcsema/BC/Util.cpp:139-142`), then runs SROA and mem2reg
  (`BC/Optimize.cpp:1623-1624`). A global can't be promoted across opaque
  calls, so it gets little.
- **Anvill** (`9948d26`) does what our spike does, but at the function's
  native interface.
  - The wrapper stack-allocates a `State` (`lib/Lifters/FunctionLifter.cpp:1153-1186`),
    stores the native arguments into it per the function's specification,
    inlines everything, and runs SROA/mem2reg (l.1468-1478).
  - A call to another function loads *only the parameter registers* from
    `State` and stores back *only the return registers*
    (`lib/Declarations.cpp:98-180`). Every other register is dropped across
    the call.
- **Exact for us?** The local-`State`-plus-SROA part, yes: it is our
  spike. The ABI part, no: Anvill trusts the specification at every call,
  which breaks on hand-written asm. The earlier review found the same in
  mctoll and RetDec, which lose flags across calls.

### Console recompilers: XenonRecomp and N64Recomp

- **XenonRecomp** (`ddd128b`) has opt-in options that make PowerPC
  registers C locals (`README.md:54-69`, `recompiler_config.cpp:21-28`).
  - `non_volatile_as_local` covers r14–r31 and the non-volatile FPRs and
    vector registers (`recompiler.cpp:271-295`). `non_argument_as_local`
    covers r0, r2, r11 and r12. There are also CTR, XER, CR and the
    reserved register, and `skip_lr` never writes LR at all
    (l.644-645).
  - The locals are zero-initialised per function (`PPCRegister r{}{{}};`,
    l.2471) and **never written back**. A call is `name(ctx, base);`
    (l.397), and the `__savegpr`/`__restgpr` calls are deleted
    (l.391-394).
  - The README justifies it with "never leave the function scope" if the
    game respects the ABI, and credits these options with the biggest
    gains: about 20 MB off Sonic Unleashed's executable, and frame times
    "reduced by several milliseconds" (l.69).
  - It breaks on hand-written asm that reads a register its caller set, on
    `setjmp`/`longjmp` (only `ctx` is saved, l.376-382), on mid-asm hooks,
    and on anything that reads LR.
- **N64Recomp** (`ffb39cd`) keeps every GPR and FPR in `ctx`
  (`src/cgenerator.cpp:81-86`). Only `hi`, `lo` and a few temporaries are
  locals, because they "shouldn't need to be preserved across function
  boundaries" (l.396-401).
- **Exact for us?** No. Deleting the saves of callee-saved registers
  changes guest memory (the stack), and nothing checks the ABI bet. They
  are the evidence of upside, not a technique to copy.

### Dynamic recompilers' register caches: Dolphin and gpsp

- **Dolphin's Jit64** (`c12121f`) caches PowerPC registers in host
  registers. `FlushMode::Full` writes back and evicts, and `Undirty`
  writes back and keeps the copy (`Jit64/RegCache/JitRegCache.h:121-128`).
  - Exits to the interpreter and HLE calls flush fully
    (`Jit64/Jit.cpp:359-365, 429-430`).
  - Exception and interrupt exits fork the cache and flush in *far code*
    (l.410-425, 1055-1068), so only the exit path pays and the hot path
    keeps its registers.
  - Memory slow paths save only host caller-saved registers, not guest
    state (`Jit_LoadStore.cpp:220, 378-384`).
- **gpsp** (`5819380`, a GBA dynarec) caches a fixed, profile-chosen set:
  r0, r1, r6, r9, r12 and r14 in ARM mode, r0–r5 in Thumb
  (`arm/arm_emit.h:138-235`, with the usage table). It stores and reloads
  them around every C call: CPSR writes, mode switches in SWI and VRAM
  interpretation (`arm/arm_stub.S:329-332, 405-407, 431-439`).
- **Exact for us?** Yes. Both are the same discipline as our table, and
  Dolphin's "flush in far code" is exactly writing back on the cold yield
  path. mGBA has no JIT to compare with.

### What it buys, where measured

Only one source measures register promotion on its own, and it is a
dynamic translator.

- **Instrew** (Engelke, Okwieka and Schulz, "Efficient LLVM-Based Dynamic
  Binary Translation", VEE 2021) lifts with LLVM. Its baseline already
  loads registers into SSA at each chunk's entry and writes back the
  modified ones before return, as our spike does.
  - Its HHVM calling convention also keeps 12 guest registers in host
    registers *across* chunk transitions. That cut SPEC CPU2017 run time by
    14% (geomean 1.96× → 1.69× native, Table 1, p.5; mcf 1.84 → 1.36),
    for 12% more translation time (782 s → 875 s).
  - On AArch64 hosts the authors put part of the remaining overhead down to
    "the entire register state [being] written back to memory".
- **BinRec** (Altinay et al., EuroSys 2020) promotes global CPU state to
  locals and tells alias analysis registers don't alias memory (§4.2). Its
  speedup is shown only as a chart (Fig. 3), not separated from its other
  optimisations.
- **XenonRecomp's** 20 MB and "several milliseconds" is the console-side
  figure, for the unexact version.
- Not obtained: LLBT (CASES 2012), Polynima, any McSema or Remill ablation.

### Stack slots

The topic asks about stack slots too. For us they are guest memory, and
that settles most of it.

- A push, a pop, an `ldr`/`str` sp-relative are bus accesses with wait
  states, and their bytes are visible to an IRQ handler, DMA (which can
  target IWRAM) and anything holding a pointer into the frame. Keeping a
  slot only in a local is not exact. Every store must happen, in order.
- What *is* exact is **forwarding a stack load** from an earlier store in
  the same function, if nothing between can write the slot: no tick (DMA
  and IRQs run there), no call, and no store that might alias. Every block
  exit has a tick, so in practice that means within a block, the same
  limit as today's forwarding. The load still has to run for its wait
  states, or be replaced by its known cost (IWRAM is a constant one
  cycle). That is as much work as it saves.
- And there is little to win. Emerald has 8,919 sp-relative stores and
  4,053 sp-relative loads, 1.6% of its instructions. agbcc keeps locals
  in registers, and its stack traffic is the 41,965 pushes and pops of
  callee-saved registers, which must reach memory anyway.

RetDec turns every `sp + const` address into its own alloca
(`optimizations/stack/stack.cpp:131-260`), a no-alias assumption. Van
Emmerik subscripts only stack slots whose address doesn't escape, and
leaves escape analysis as future work (thesis §4.2.3, p.118). Neither is
exact for us.

### Registers in locals: exact, and at what cost

| Technique | Used by | Exact for us? | Cost |
|---|---|---|---|
| Every register in `Machine`, every write a store | **ours** (v1), N64Recomp, Remill | Yes (baseline) | 687k stores and 197k loads in Emerald's code; LLVM can't keep a register across a block |
| Drop writes overwritten later in the same block | QEMU (within a TB), LLVM DSE where the bus inlines | Yes: nothing between reads `cpu.r` | A builder change; removes 307k statements (45% of register writes) |
| Locals for the function, LLVM promotes, write back at every boundary in the table | the spike, Instrew's baseline, Anvill's `State` alloca, Dolphin, gpsp | **Yes**, by construction, with no ABI assumption | A write-back and reload per call and per mode switch; write-backs on the cold yield path; φs at v1's loop head |
| Our own SSA (Braun) over the function | libFirm, Braun's LLVM prototype | Yes | Linear; only worth it for cross-block analyses, not for promotion |
| TCG-style classification of every runtime call (no-read, sync, kill) | QEMU | Yes, if honest | One table, which "Where state must be in Machine" is |
| Locals never written back, ABI trusted (no saves, no LR) | XenonRecomp, Anvill, mctoll, RetDec | **No** | — |
| Pass registers across direct calls as host arguments and results (a custom host convention) | Instrew (HHVM convention) | Yes, if the callee still writes back at its own boundaries | Instrew: 14% less run time, 12% more compile time. Rust has no such convention; a 15-value result goes through memory |
| Stack slots in locals | RetDec, Hex-Rays, Van Emmerik | **No**: the stack is guest memory | — |
| Forward a stack load from a store in the same block | — | Yes, keeping the access's wait states | Little to win: sp loads are 0.5% of instructions |

## Calling conventions and frame recovery

Decision 10 wants summaries: what a function reads at entry, clobbers and
preserves. The first question is what they would buy, given the table
above.

### What a checked summary buys, exactly

With registers in locals, a direct call in safe mode writes back every
register the caller holds and reloads every register after. A summary can
shrink both sides:

- **Before the call**, the caller must write back every register the
  callee may read. That includes the callee-saved registers it pushes: the
  push stores the caller's values to guest memory, so the callee needs
  them exactly. For a typical agbcc function, `push {r4-r7, lr}`, the
  read set is r0–r3 (arguments), r4–r7 and lr.
- **After the call**, the caller must reload every register the callee may
  write. A *proved* preservation of r4–r11 and sp means the caller's locals
  are still right, so no reload.
- **On a yield inside the callee**, nothing changes: the callee writes back
  its own locals and returns, and the caller returns without writing back
  (the table's fifth row). So summaries can't make the yield path wrong.

So the most a summary saves at a call is the write-back of registers the
callee never touches and the reload of registers it preserves: perhaps a
dozen host loads and stores per call, at 53,500 call sites. That is real
but small next to promotion inside functions, which needs no summary at
all. Decision 10's "safe mode" and "trusting mode" turn out to be two
levels of one thing, both with registers in locals.

What summaries can't do for us is what they do for XenonRecomp and Anvill:
delete the callee's pushes and pops. The stack is guest memory, so those
stores and loads stay.

### Van Emmerik's preservation analysis (Boomerang)

Mike Van Emmerik's thesis, *Static Single Assignment for Decompilation*
(University of Queensland, 2007), is the closest prior art to decision 10.

- **Per procedure**, it infers parameters (live on entry, after a
  parameter filter: Eq. 3.6, §3.4.4, p.84), modifieds (definitions
  reaching the exit, minus preserveds: Eq. 3.8) and returns (modifieds
  live at *some* call site, Eq. 3.11, p.85). So returns depend on the
  whole program (§3.5).
- **Preservation** of a location x is the equation `x0 = x_exit`, solved by
  substitution and simplification over SSA (§4.3, pp.121-123). At a φ the
  goal splits, and every operand must prove it.
- **Bottom-up** over the call graph, with recursion found as strongly
  connected components (Alg. 3, p.131). An unanalysed or indirect callee is
  `<all> := CALL f(<all>)`: it reads and writes everything (§4.4, p.127).
- **Recursion** uses premises (§4.4.2, pp.130-133). To prove x preserved in
  p, assume it at p's recursive calls; every further premise must hold, or
  the whole goal fails. That is sound for an all-paths property.
- **The stack pointer** is the one location exempt from "live on entry
  means parameter" (§3.4.5). Its lattice has "preserved + k" (Fig. 4.28,
  p.134).
- **Safety.** Over-estimating definitions and uses is safe; under-estimating
  either is not (§3.6, pp.90-92). He argues against trusting an ABI,
  because of hand-written asm and binary patches (§4.3, p.120).

The design is right for us, but the implementation is not sound enough to
copy. A restore from a stack slot counts as preserving only if nothing
between the save and the restore can write that slot. Van Emmerik leaves
that as future work (§4.2.3, p.118). Boomerang's prover
(`src/boomerang/db/proc/UserProc.cpp`, at `4110413`) strips `m[…]` from
both sides with no alias check (l.1522-1527), substitutes any matching
statement with no dominance check (l.1540-1556, "FIXME: this seems pretty
much like a bad hack!"), and takes a φ loop as true "due to induction"
(l.1453).

### rev.ng: folding exit minus entry

rev.ng (`352c75e`) is the one tool that checks preservation by computing
it. `lib/EarlyFunctionAnalysis/CFGAnalyzer.cpp:1080-1098` outlines each
function, puts a marker at every exit recording `reg_exit − reg_entry` for
sp, the return address and each ABI register, and runs LLVM's optimiser.
A register whose difference folds to the constant 0 is callee-saved;
anything else is clobbered.

- The final stack offset is elected by vote among the returns
  (l.800-960). An exit with a different offset is reclassified as a broken
  return or a `longjmp`, not rejected.
- Callees inside the function are replaced by their summary, which is the
  callee's prototype, or the binary's *default* prototype when there is
  none (`FunctionSummaryOracle.cpp:150-153`). Callee-saved registers are
  taken from the prototype (l.18-35): an ABI assumption.
- Arguments and returns come from liveness and reaching definitions, then
  `applyABIDeductions` forces the result into the ABI's shape at the
  default `abi-enforcement-level=full` (`DetectABI.cpp:64-91, 713-790`).
- Its AAPCS definition (`share/revng/abi/AAPCS.yml:46-58`) lists r4–r8,
  r10, r11 and r14 as callee-saved: it leaves out r9 and adds lr, unlike
  AAPCS §6.1.1.
- The CC 2017 paper covers CFG and function-boundary recovery only; no
  rev.ng publication on calling-convention detection was found.

The fold-to-zero check is adoptable as is, run on our own IR with our
exact semantics. Its weakness is the same as Van Emmerik's: LLVM folds
`load(store(sp+k, r4))` back to `r4` only if it can prove nothing between
aliases the slot. And the default prototypes and ABI enforcement are
guesses.

### Ghidra, RetDec, Hex-Rays: trust the table

- **Ghidra** (`1640030`). A compiler spec lists registers as `unaffected`
  (the callee keeps them) or `killedbycall`; anything else is unknown
  (`fspec.hh:396-403`). At a call, heritage guards unknown registers with an
  `INDIRECT` op, but "we do not guard the call if the effect is
  'unaffected'" (`heritage.cc:1468, 1511-1527`). So unaffected registers
  flow across every call on trust.
  - Parameters come from *trials* over the model's own parameter slots
    (`possibleInputParam`, `coreaction.cc:5050-5100`), plus heuristics
    such as filling holes before a chain of inactive slots
    (`fspec.cc:1088-1100, 1272-1300`). A hand-written routine that reads r5
    at entry can never get r5 as a parameter.
  - With a fixed `extrapop`, `sp = sp + 0` is inserted after every call
    unchecked (`coreaction.cc:1510-1535`).
  - When a guess fails, Ghidra prints a warning in the output ("Prototype
    may be inaccurate", `coreaction.cc:5244-5290`).
  - The ARMv4T language uses `ARM_v45.cspec` (`ARM.ldefs:383-390`):
    unaffected r4–r11 and sp, killedbycall only r1. So r2, r3, r12 and lr
    are "unknown". Both ARM specs say "Assumes … AAPCS Applies".
- **RetDec** (`9450585`). In its reaching definitions a call only *uses*
  its operands and never defines anything
  (`analyses/reaching_definitions.cpp:174-195`, with "TODO … is this
  ok?"), so every register survives every call. Register localization then
  makes every register a per-function alloca unconditionally
  (`register_localization.cpp:58-110`). Arguments are filtered to the
  convention's r0–r3 (`param_return/filter.cpp:1206-1257`).
- **Hex-Rays** (closed). Guilfanov's "Decompilers and beyond" (Black Hat
  2008, pp.1-2) says the questions of a call's inputs, spoiled registers
  and sp change "can not [be answered] with absolute certainty", so it
  makes "configurable assumptions" and uses "heuristics for unsolvable
  problems". His post "Some functions are neater than the decompiler
  thinks" (2008-04-09): it "assumes that functions obey the regular calling
  conventions", the user fixes exceptions with `__usercall` and `__spoils`,
  and "when they are wrong… the output does not correspond to the input".
  Its one hard check is sp balance ("sp-analysis failed", "positive sp
  value": docs.hex-rays.com, decompiler failures).

None of the three checks anything at a call. For readable output a wrong
guess is a warning; for us it is a wrong frame. They are useful only as an
oracle to compare our summaries against.

### What the ABI actually is, for agbcc

- **AAPCS32** (ARM IHI 0042, `abi-aa` at `17f2cc8`): a subroutine "must
  preserve r4-r8, r10, r11 and SP (and r9 in PCS variants that designate r9
  as v6)" (`aapcs32.rst:866-881`).
  - r12 may be used by the linker as scratch (l.858). "Any veneer inserted
    must preserve the contents of all registers except IP (r12) and the
    condition code flags", and code must assume one at any interworking or
    long branch (l.1206-1215).
  - sp is 4-byte aligned always, 8-byte only at a public interface
    (l.1061, 1085). Flags are undefined across a call.
- **ATPCS** (SWS ESPC 0002 B-01, 2000), the convention of agbcc's time:
  r4–r11 and sp preserved (p.11). The 8-byte alignment was "added in B-01,
  motivated by ARM architecture 5"; before that, 4.
- **agbcc** (`pret/agbcc` at `da598c1`, `gcc/thumb.h`):
  `STACK_BOUNDARY 32` (l.355). `FIXED_REGISTERS` fixes r11, sp, lr and pc;
  `CALL_USED_REGISTERS` is r0–r3 and r11–r15 (l.396-411). So r4–r10 are
  callee-saved and r11 is never allocated. Its interworking epilogue
  (`thumb.c:556-620`) pops the return address into whichever of r0–r3 the
  return value doesn't use. `_call_via_rX` is a bare `bx rX`
  (`libgcc/lib1thumb.asm:606-614`).

So the spec at an open edge is: r4–r10 and sp preserved (r11 too, since
agbcc never touches it, but not by its own rule); r0–r3, r12, lr and NZCV
clobbered; sp aligned to 4; return by `bx` in the caller's state. Neither
Ghidra's ARMv4T spec nor rev.ng's AAPCS.yml matches it.

### Checking summaries, from other binary tools

- **TypeArmor** (van der Veen et al., S&P 2016) computes, per callee, which
  argument registers are read before written on *all* paths (§IV-A2), and
  treats an unresolved indirect call as writing all arguments (§IV-A1). It
  under-approximates reads on purpose, which suits CFI and is the opposite
  of what exactness needs: for us a read on any path is a read. It puts
  hand-written binaries out of scope (§II-A).
- **Spike** (Cohn, Goodwin and Lowney, Digital Technical Journal 1998), an
  executable optimiser, "performs an interprocedural dataflow analysis to
  summarize register usage within the image", with memory analysed only for
  stack references. No algorithm is given. Goodwin's PLDI 1997 paper on it
  was not obtained.
- Not obtained: τCFI, Diablo/ALTO, BOLT's register analysis.

### Is it exact, and what it costs

| Technique | Exact for us? | Cost |
|---|---|---|
| Trust the ABI at every call (Ghidra, RetDec, Hex-Rays, Anvill, XenonRecomp) | **No**: hand-written asm, IRQ entry, `_call_via_rX`, the m4a mixer | — |
| Fixed AAPCS spec, checked per function (decision 10's open edges) | Yes, if the check proves preservation over all paths and fails to safe mode | A preservation prover |
| Van Emmerik's bottom-up preservation with premises for recursion | Yes, with a sound memory step (no store between save and restore may alias the slot) | A simplifier over our IR, SCC order, a premise map, a frame-alias check |
| rev.ng's fold `exit − entry` to 0 | Yes on our own IR, with the same alias caveat; its ABI enforcement and default prototypes, no | The same, with LLVM or our own folding as the prover |
| Read sets by may-read over any path (the opposite of TypeArmor's) | Yes | Liveness we already have the shape of (flag liveness) |

The alias step is the hard one on the GBA. The stack is in IWRAM with
everything else hot, so a store through an unknown pointer between a
push and its pop might hit the save slot. The check has to prove the store
can't (value sets: the store's address is outside the frame) or fail the
function to safe mode. Precision, not correctness, is what fails there.

## Type recovery

The last question: would knowing types (this register is a pointer to a
struct, that one a `u8`) let codegen emit faster or smaller code that is
still exact? The memory side matters most, since every access goes
through `m.bus.load`/`store`. Today those already inline a guarded fast
path for EWRAM and IWRAM, `match addr >> 24 { 0x02 | 0x03 => …, _ => slow }`
(`runtime/src/bus.rs:505-524`), and call one non-inlined function for the
rest of the map.

### TIE (Lee, Avgerinos and Brumley, NDSS 2011)

- **Lattice:** `reg32_t` at the top, then `num32_t`, `ptr(α)` and
  `code_t`, then signed and unsigned integers; records, intersections and
  unions for structures (Figs. 5–6, p.8). Each variable gets an interval,
  an upper and a lower bound.
- **Constraints** are generated per statement (Fig. 7, p.10). An addition
  is a three-way *disjunction*: number plus number, pointer plus number or
  number plus pointer. Memory needs a value-set analysis (DVSA, §5.2), whose
  memory equalities the authors call "imprecise and possibly
  un-conservative" (§6.2, p.11). Solving unifies equalities, propagates
  bounds and merges disjunctions "conservatively" (§6.3).
- **Soundness** is against the *source* types: "conservative" means the
  bounds bracket the declared type (Def. 6.1), measured "typically above
  90%" (p.11). Nothing is claimed about behaviour. No run times are given.

### Retypd (Noonan, Loginov and Cok, PLDI 2016)

- **Constraints** are subtyping over *derived type variables*: `X.load`,
  `X.store`, `F.in`, `F.out` and `.σN@k` (an N-bit field at offset k)
  (Table 1, p.5). A load gives `P.load.σ32@0 ⊑ X`. The rule
  `α.store ⊑ α.load` is "a sound account of pointer subtyping" (§3.3).
- **Solving** treats the constraints as a pushdown system and saturates it,
  in O(|C|³) (Theorem 5.1, p.9), per call-graph SCC bottom-up, then
  specialises top-down (§4.2).
- **Soundness** is relative to the constraint set. The paper says
  "disassembly failures can lead to unsound constraint generation" (§6.3)
  and the translation to C types is "heuristic … lossy by necessity"
  (§4.3). Conservativeness is 95% on average (§6.5).
- **Cost:** about 0.000725·N^1.098 s for N CFG nodes, with value-set
  analysis turned off (§6.6, Figs. 11–12). For Emerald's 815k
  instructions that extrapolates to about 40 minutes, as an extra
  stage.
- **The implementation** (`GrammaTech/retypd` at `8f7f72b`) has no
  constraint generator of its own, and its docs say the steps after
  saturation "reflect recent updates … and not the original paper", with
  "I have not yet proven" on one of them (`reference/type-recovery.rst:147-195`).

### Ghidra and Hex-Rays

- **Ghidra** (`1640030`). `ActionInferTypes` (`coreaction.cc:5736-5779`)
  seeds types from each op (`typeop.cc`) and propagates them along
  data flow, stopping at user-locked types (`coreaction.cc:5454`). It gives
  up after seven rounds, "a constant arrived at empirically", with the
  warning "Type propagation algorithm not settling" (l.5750-5757).
  - The rewrites it drives are semantically neutral: `PTRADD` and `PTRSUB`
    are the same arithmetic (`opbehavior.cc:769-781`) and `CAST` is a no-op
    (l.77). `RulePtrArith` and `RuleStructOffset0` only reshape
    expressions (`ruleaction.cc:6497-6620`).
  - **One isn't:** `SplitDatatype` (`subflow.cc:2761-2790`, with
    `RuleSplitLoad`/`RuleSplitStore` at l.2983-3010) splits one load or
    store into narrower ones because of an inferred struct type. On the GBA
    that changes behaviour: a 32-bit I/O access is two 16-bit ones with
    their own effects, VRAM and OAM ignore or widen byte stores, and the
    wait states differ.
- **Hex-Rays** (closed; its SDK header and Guilfanov's RECON 2018 slides).
  Types are assigned after the microcode is final: `MMAT_GLBOPT3`,
  "microcode is fixed now", comes before `MMAT_LVARS`, "allocated local
  variables" (`hexrays.hpp:4877-4888`). Guessed types are flagged as guessed
  (`WARN_GUESSED_TYPE`). Its stack promotion treats every stack slot below
  the lowest address-taken one (`minstkref`) as a non-aliased register;
  the slide says "yes, this is one of the assumptions our decompiler
  makes" (slides 52–54).

### Value sets, and the newer systems

- **Value-set analysis** (Balakrishnan and Reps, "WYSINWYX", TOPLAS 2010)
  is the memory-region analysis the type systems sit on: memory is split
  into a global region, one per activation record and one per allocation
  site (§2.1), with strided intervals per region. It is "sound, as long as
  it can provide warnings" about frame overruns, unresolved indirect
  jumps and self-modifying code (§3.8). On the GBA the regions are fixed
  address ranges, so a strided interval over absolute addresses (plus
  sp-relative offsets) names the region already. That is what our
  `analysis::vsa` does for jump tables (decision 5c).
- **OSPREY** (S&P 2021) is probabilistic (belief propagation); **DIRTY**
  (USENIX Security 2022) is a Transformer, 75.8% of types right;
  **TRex** (USENIX Security 2025) claims "conditional conservativeness"
  only; **BinSub** (SAS 2024) is Retypd-like algebraic subtyping, 63×
  faster (0.024 s per function). None says anything about behaviour.

### Where regions do buy speed, elsewhere

- **Lasagne** (PLDI 2022; read from the authors' slides, the paper wasn't
  obtained) gets its gain from a *region* fact, not a type: "if the access
  is performed on a stack address, then no fence is inserted" (slide 27),
  up to about 65% fewer fences.
- **rev.ng's "TypeShrinking"** is LLVM demanded-bits plus value ranges
  (`include/revng/TypeShrinking/TypeShrinkingAnalysis.h`): the sound way
  to narrow a `u32` to a `u8`, and something LLVM already does to our code.
- **mGBA** (`fd9dc5c`) dispatches every access on `address >> 24`
  (`src/gba/memory.c:476-521`), with wait states that change at run time
  for EWRAM and VRAM (`memory.c:396-421`) and none for IWRAM (l.400).
  **gpsp** jumps on `addr >> 24` to direct host accesses for IWRAM and
  EWRAM (`x86/x86_stub.S:332-355`) and charges wait states only in its
  interpreter (`cpu.cc:864-945`), so it isn't exact.

### Is it exact, and what it buys

| Technique | Exact for us? | What it buys | Cost |
|---|---|---|---|
| TIE, Retypd, BinSub (constraint types) | Only as hints: sound against constraints, not behaviour | Nothing in codegen: a `ptr` type doesn't say which region | Retypd ~N^1.1 s; about 40 min on Emerald, extrapolated |
| OSPREY, DIRTY, TRex (probabilistic or conditional) | No | — | — |
| Ghidra's type propagation and pointer rewrites | Neutral (presentation) | Readable output only | — |
| Width splitting from struct types (Ghidra `SplitDatatype`) | **No**: I/O, VRAM, OAM and wait states | — | — |
| Hex-Rays `minstkref` stack promotion | **No** (assumed, not proved) | — | — |
| Guarded region fast path, `addr >> 24` | Yes | **Built** (`bus.rs:505-524`) | — |
| Proved region (value sets, sp-relative with sp proved in IWRAM) | Yes, if proved | Saves the `>> 24` compare and branch on a fast path that is already inline; IWRAM has a constant cost | A per-function sp proof, as hard as the preservation check |
| Narrower integer types | Yes (LLVM's demanded bits) | Already done by LLVM | — |

So type recovery buys nothing that is exact. It changes no semantics we
emit, and the one transformation that a type drives (width splitting) is
wrong on the GBA. Region facts are the useful part, and our bus already
takes the cheap, guarded version of them. A proved region would save one
compare per access, and only where the proof is as hard as the stack
check above. Types stay where references.md put Retypd: "for the long-term
readable-output direction".

## What our design should take

For `structured/design`:

1. **Drop in-block overwritten register writes now, in safe mode.** It is
   exact (nothing between the two writes reads `cpu.r`), it needs no
   locals, and it removes 307k statements from Emerald. The IR builder
   already forwards reads within the block; it should also emit only each
   register's last write before the next boundary in the table (an exit,
   a call, a `swi`, an `msr`, a user-bank op). Check: `just sst-gen`, the
   recompile checks and Emerald's frame hashes, unchanged.
2. **Then registers and NZCV as function locals, promoted by LLVM.**
   - Hold only the registers the function reads or writes, and write back
     only those it writes (the spike's one waste).
   - Constant indices or named locals only, never a computed register
     index; unroll `ldm`/`stm` lists (they are already).
   - Load at function entry, whatever the key, so the structured body and
     the v1 resume body (structured-control-flow.md, item 3) share it.
   - Write back and reload exactly per "Where state must be in `Machine`".
     Make that table one function in the printer that every runtime call
     goes through, as QEMU's helper flags are, so a new `rt::` helper
     can't be added without saying what it reads.
   - After a call returns with `rt::yielding`, return without writing
     back.
   - The I bit and the mode stay in `cpu.cpsr`; only N, Z, C and V become
     locals.
3. **Keep decision 9's flag liveness inside a block; measure it across
   blocks.** With flag locals, the yield path's write-back composes the
   exact CPSR, which closes the `IntrMain` gap. But a cross-block-dead
   flag write would then be a statement again, costing rustc's front end,
   even though LLVM sinks it. Count what cross-block liveness removes
   today, before deciding which way to go.
4. **Spike before building, on one part crate** (p07: 20k instructions,
   1.13 GB and 56 s today), generated three ways: v1, item 1, items 1–2.
   Measure rustc's time and peak memory, the object size, and the run
   time of a fixed Emerald route (there is no profiler on quack: install
   `perf`, or time frames). The frame hashes must match all three ways.
5. **Re-scope decision 10.** Promotion needs no ABI trust, so "safe mode"
   becomes registers in locals with full write-back at calls. Summaries
   only shrink a call's write-back and reload sets. Build them only if the
   spike shows calls matter, and then:
   - bottom-up over call-graph SCCs, `<all>` for anything unknown, premises
     for recursion (Van Emmerik);
   - read sets by may-read on any path (not TypeArmor's all-paths);
   - preservation proved as `r_exit = r_entry` for r4–r10 and sp over all
     paths (rev.ng's check), counting a restore from a stack slot only if
     value sets prove no store between can hit the slot;
   - at open edges, agbcc's convention as the fixed spec, checked;
   - every failure stays in safe mode. Ghidra's and IDA's prototypes can
     be an oracle to compare against, never an input.
6. **No stack-slot promotion and no type recovery in codegen.** Neither
   is exact for us, and the sources show nothing to gain that the
   guarded bus fast path doesn't already take. Revisit types only for
   readable output.
7. **The IRQ path, when it stops unwinding** (static-recompilers.md, item
   3): write back before running the nested handler, reload after. A
   save-state *load* while compiled frames are on the host stack must
   unwind them all, since every frame's locals are stale.
8. **For `structured/lit-validation`:**
   - A printer-level check that every runtime call is preceded by the
     write-back its row requires.
   - A memory compare at checkpoints, not only the frame hash. A missed
     write-back of a dead register shows up only in memory, on the IRQ
     stack.
   - A safe-versus-locals diff of the whole `Machine` at every dispatcher
     entry (decision 10's own oracle).

## Sources

Read from the primary text or source, on 2026-10-24. Source trees at the
commit given (shallow or sparse clones under `/tmp/lit-registers`, never
built or run).

SSA and promotion:

- Matthias Braun, Sebastian Buchwald, Sebastian Hack, Roland Leißa,
  Christoph Mallon, Andreas Zwinkau, "Simple and Efficient Construction
  of Static Single Assignment Form", CC 2013, LNCS 7791
  (c9x.me/compile/bib/braun13cc.pdf).
- Ron Cytron, Jeanne Ferrante, Barry K. Rosen, Mark N. Wegman, F. Kenneth
  Zadeck, "Efficiently Computing Static Single Assignment Form and the
  Control Dependence Graph", TOPLAS 13(4), 1991.
- LLVM, `llvm/lib/Transforms/Utils/PromoteMemoryToRegister.cpp`,
  `llvm/lib/Transforms/Scalar/SROA.cpp`, `llvm/lib/Analysis/PtrUseVisitor.cpp`,
  github.com/llvm/llvm-project at `bfe0941`.
- QEMU, `tcg/tcg.c`, `include/tcg/tcg.h`, `include/tcg/tcg-opc.h`,
  `target/arm/tcg/`, `accel/tcg/translate-all.c`, github.com/qemu/qemu at
  `d731492`.
- Remill, github.com/lifting-bits/remill at `56918a8`; McSema,
  github.com/lifting-bits/mcsema at `2636805`; Anvill,
  github.com/lifting-bits/anvill at `9948d26`.
- XenonRecomp, github.com/hedge-dev/XenonRecomp at `ddd128b`; N64Recomp,
  github.com/N64Recomp/N64Recomp at `ffb39cd`.
- Dolphin, `Source/Core/Core/PowerPC/Jit64/`, github.com/dolphin-emu/dolphin
  at `c12121f`; gpsp, github.com/libretro/gpsp at `5819380`.
- Alexis Engelke, Martin Schulz, "Instrew: Leveraging LLVM for High
  Performance Dynamic Binary Instrumentation", VEE 2020; Alexis Engelke,
  Dominik Okwieka, Martin Schulz, "Efficient LLVM-Based Dynamic Binary
  Translation", VEE 2021.
- Anil Altinay et al., "BinRec: Dynamic Binary Lifting and Recompilation",
  EuroSys 2020.

Calling conventions and frames:

- Mike Van Emmerik, *Static Single Assignment for Decompilation*, PhD
  thesis, University of Queensland, 2007; Boomerang,
  github.com/BoomerangDecompiler/boomerang at `4110413`.
- Ghidra decompiler (`Ghidra/Features/Decompiler/src/decompile/cpp`) and
  ARM language specs (`Ghidra/Processors/ARM/data/languages/`),
  github.com/NationalSecurityAgency/ghidra at `1640030`.
- RetDec, github.com/avast/retdec at `9450585`.
- rev.ng, github.com/revng/revng at `352c75e`; Alessandro Di Federico,
  Mathias Payer, Giovanni Agosta, "rev.ng: A Unified Binary Analysis
  Framework to Recover CFGs and Function Boundaries", CC 2017.
- Ilfak Guilfanov, "Decompilers and beyond", Black Hat USA 2008 (white
  paper); "Some functions are neater than the decompiler thinks",
  hex-rays.com blog, 2008-04-09; Hex-Rays, "Decompiler failures",
  docs.hex-rays.com.
- ARM, *Procedure Call Standard for the Arm Architecture* (AAPCS32, IHI
  0042), github.com/ARM-software/abi-aa at `17f2cc8`; ARM, *The ARM-Thumb
  Procedure Call Standard* (ATPCS), SWS ESPC 0002 B-01, 2000.
- agbcc, github.com/pret/agbcc at `da598c1` (`gcc/thumb.h`, `gcc/thumb.c`,
  `libgcc/lib1thumb.asm`).
- Victor van der Veen et al., "A Tough Call: Mitigating Advanced
  Code-Reuse Attacks at the Binary Level" (TypeArmor), S&P 2016.
- Robert Cohn, David Goodwin, P. Geoffrey Lowney, "Optimizing Alpha
  Executables on Windows NT with Spike", Digital Technical Journal 9(4).

Types and regions:

- JongHyup Lee, Thanassis Avgerinos, David Brumley, "TIE: Principled
  Reverse Engineering of Types in Binary Programs", NDSS 2011.
- Matt Noonan, Alexey Loginov, David Cok, "Polymorphic Type Inference for
  Machine Code", PLDI 2016 (arXiv 1603.05495); retypd,
  github.com/GrammaTech/retypd at `8f7f72b`.
- Gogul Balakrishnan, Thomas Reps, "WYSINWYX: What You See Is Not What You
  eXecute", TOPLAS 32(6), 2010.
- Zhuo Zhang et al., "OSPREY: Recovery of Variable and Data Structure via
  Probabilistic Analysis for Stripped Binary", S&P 2021; Qibin Chen et al.,
  "Augmenting Decompiler Output with Learned Variable Names and Types"
  (DIRTY), USENIX Security 2022; Ian Smith, "BinSub: The Simple Essence of
  Polymorphic Type Inference for Machine Code", SAS 2024 (arXiv
  2409.01841); TRex, USENIX Security 2025.
- Hex-Rays SDK, `hexrays.hpp`, github.com/HexRaysSA/ida-sdk at `f80b128`;
  Ilfak Guilfanov, "Hex-Rays microcode API", RECON 2018 (slides).
- Rodrigo C. O. Rocha et al., "Lasagne: A Static Binary Translator for
  Weak Memory Model Architectures", PLDI 2022 (the authors' slides; the
  paper was not obtained).
- mGBA, `src/gba/memory.c`, github.com/mgba-emu/mgba at `fd9dc5c`.

Not obtained: Goodwin, "Interprocedural Dataflow Analysis in an Executable
Optimizer", PLDI 1997 (ACM); LLBT (Shen et al., CASES 2012) beyond its
abstract; Polynima; the Lasagne paper; a rev.ng publication on
calling-convention detection (none found; the nearest is Di Federico's
commit introducing the stack analysis, revamb `a5af286`, 2017). Our own
code: `crates/codegen`, `crates/analysis/src/ir/`,
`crates/runtime/src/{dispatch,sched,bus}.rs`, and pokeemerald
(`$GBAA_DATA/src/pokeemerald`: `pokeemerald.map`, `src/crt0.s`).

## Reproducing

The counts in "Measured" come from a one-off scan of a generated
`out/pokemon-emerald-version` (about 20 s, `python3 -I scan.py
out/pokemon-emerald-version`). For each match arm it counts
`m.cpu.r[i] = ` (writes) and other `m.cpu.r[i]` (reads). It calls a write
dead when a later write to the same register sits at the arm's top level
with no read of that register, no `return`, `continue`, `rt::exit`,
`rt::swi`, `rt::rekey`, call or other `m.cpu.` method between, and no
nested `pc =`. Calls are `f_X(m, ` sites. A function has no direct caller
when no other function's text calls it. It is a text heuristic, checked by
hand on a sample (the example in "What we start from" is one), not a
proof:

```python
# One-off count over generated Emerald: register traffic, calls, dead in-block writes.
import re, sys, os, collections
root = sys.argv[1]
files = []
for d in ("src", "parts"):
    for dp, _, fs in os.walk(os.path.join(root, d)):
        files += [os.path.join(dp, f) for f in fs if f.endswith(".rs") and f != "lib.rs"]
fn_re = re.compile(r"^pub fn (f_[0-9a-f]+)\(")
arm_re = re.compile(r"^ {12}0x[0-9a-f_]+ => \{")
w_re = re.compile(r"m\.cpu\.r\[(\d+)\] = ")
r_re = re.compile(r"m\.cpu\.r\[(\d+)\](?! = )")
call_re = re.compile(r"\b(f_[0-9a-f]+)\(m, ")
barrier = re.compile(r"return |continue|rt::exit|rt::swi|rt::rekey|rt::call|\bcall\(m|m\.cpu\.(?!r\[|set_flag|cpsr =|cpsr &|flag\()")
S = collections.Counter()
reads = collections.Counter(); writes = collections.Counter()
callers = collections.defaultdict(set); fns = set()
arm_reads_first = collections.Counter()  # reads of a reg in a block before any write to it there
for path in files:
    cur = None; pending = {}; written = set()
    for line in open(path):
        m = fn_re.match(line)
        if m:
            cur = m.group(1); fns.add(cur); S["fns"] += 1; continue
        if arm_re.match(line):
            S["blocks"] += 1; pending = {}; written = set(); continue
        ind = len(line) - len(line.lstrip(" "))
        for c in call_re.findall(line):
            callers[c].add(cur); S["call_sites"] += 1
        rs = r_re.findall(line); ws = w_re.findall(line)
        for r in rs:
            reads[int(r)] += 1; pending.pop(r, None)
            if r not in written: arm_reads_first[int(r)] += 1
        if barrier.search(line) or (ind > 16 and ("pc =" in line)):
            pending = {}
        for r in ws:
            writes[int(r)] += 1; written.add(r)
            if ind == 16:
                if r in pending: S["dead_inblock_writes"] += 1
                pending[r] = True
            else:
                pending.pop(r, None)
S["reads"] = sum(reads.values()); S["writes"] = sum(writes.values())
S["block_entry_reads"] = sum(arm_reads_first.values())
nocall = [f for f in fns if not callers.get(f)]
S["fns_no_direct_caller"] = len(nocall)
S["fns_one_caller_fn"] = sum(1 for f in fns if len(callers.get(f, ())) == 1)
for k, v in S.items(): print(f"{k:24} {v}")
print("reads by reg ", [reads[i] for i in range(15)])
print("writes by reg", [writes[i] for i in range(15)])
```

The stack and literal counts are `grep -rhoE` over the `insn!` strings
(`"Block \{[^"]*rn: Sp`, `"(Load|Store) \{[^"]*rn: (Sp|Pc)`); the
mode-switch counts are `grep -rhoF` of `m.cpu.msr_cpsr`, `m.cpu.msr_spsr`,
`m.cpu.user_reg`, `m.cpu.set_user_reg` and `m.cpu.spsr()`. The pokeemerald
split is the map's `.text` sections, by whether the object has a `.s` or
`.c` source in the decomp tree. The LLVM observation is
`objdump -d --disassemble=gbaa_f_080a8fb1` on the built binary.

The spike is `rustc --edition 2021 -O --crate-type=lib --emit=asm
spike.rs`, then reading `today` and `locals` in `spike.s`:

```rust
// Spike: guest registers in a local array, written back only where control
// leaves. Does LLVM keep them in host registers across opaque calls?
pub struct Cpu { pub r: [u32; 15], pub cpsr: u32 }
pub struct Bus { pub ram: [u32; 64], pub clock: u64, pub next: u64 }
pub struct M { pub cpu: Cpu, pub bus: Bus }

#[inline(never)]
pub fn sync(m: &mut M) -> bool { m.bus.clock & 1 != 0 }
#[inline(never)]
pub fn io_store(m: &mut Bus, a: u32, v: u32) { m.ram[(a as usize) & 63] = v; }

#[inline(always)]
fn tick(m: &mut M, c: u32) -> bool {
    m.bus.clock += c as u64;
    if m.bus.clock < m.bus.next { return false; }
    sync(m)
}

// Today's shape: registers in m.cpu.r.
pub fn today(m: &mut M, mut pc: u32) -> u32 {
    loop {
        match pc {
            1 => {
                m.cpu.r[0] = m.cpu.r[1].wrapping_add(m.cpu.r[2]);
                io_store(&mut m.bus, m.cpu.r[3], m.cpu.r[0]);
                m.cpu.r[1] = m.cpu.r[0] << 2;
                pc = 2;
                if tick(m, 3) { return pc; }
                continue;
            }
            2 => {
                m.cpu.r[2] = m.cpu.r[1] ^ m.cpu.r[0];
                m.cpu.r[3] = m.cpu.r[3].wrapping_sub(1);
                pc = if m.cpu.r[3] != 0 { 1 } else { 3 };
                if tick(m, 2) { return pc; }
                continue;
            }
            _ => return pc,
        }
    }
}

// Locals: load at entry, write back on every way out.
pub fn locals(m: &mut M, mut pc: u32) -> u32 {
    let mut r = m.cpu.r;
    loop {
        match pc {
            1 => {
                r[0] = r[1].wrapping_add(r[2]);
                io_store(&mut m.bus, r[3], r[0]);
                r[1] = r[0] << 2;
                pc = 2;
                if tick(m, 3) { m.cpu.r = r; return pc; }
                continue;
            }
            2 => {
                r[2] = r[1] ^ r[0];
                r[3] = r[3].wrapping_sub(1);
                pc = if r[3] != 0 { 1 } else { 3 };
                if tick(m, 2) { m.cpu.r = r; return pc; }
                continue;
            }
            _ => { m.cpu.r = r; return pc; }
        }
    }
}
```

The tools' claims can be checked in their trees at the commits under
"Sources", by the file and line given.
