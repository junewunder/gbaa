# The `Sem` trait and the per-block IR

Design for decision 3. Code: `crates/armv4t/src/sem.rs` (trait, helpers),
`sem/prim.rs` (the `u32` meaning of the primitives), `sem/arm.rs` (the
ARMv4T semantics, Thumb included, and `execute`), `crates/armv4t/src/interp.rs`
(the interpreter), `crates/analysis/src/ir.rs` (the IR) and `ir/build.rs`
(the IR builder).

## The idea

Each instruction's meaning is one Rust function, generic over `S: Sem`:

```rust
pub fn bx<S: Sem>(s: &mut S, at: u32, rm: Reg) {
    let target = read_reg(s, rm, at, 8);
    s.jump_exchange(target);
}
```

Decoded fields (`rm`, the address `at`, the S bit, immediates) are plain Rust
values: static. Only `S::V` (a 32-bit value) and `S::B` (a bit) are dynamic.
A `match` or `if` on a decoded field runs once per instruction, at decode
time for the interpreter or at recompile time for the builder, and never
shows up in the IR. That's the whole binding-time story: nothing to analyse.

Interpretations:

| `S` | `V` | `B` | What running a semantic function does |
|---|---|---|---|
| `armv4t::interp` | `u32` | `bool` | executes the instruction |
| `analysis::ir::build::Builder` | `ValueId` | `ValueId` | appends its IR to a block |
| `analysis::vsa::exec::Exec` | `AbsVal` (arena index) | `Bit` (arena index) | computes the instruction's effect on an abstract state: value-set analysis for jump tables (5c) and the far-jump rule (5a) |

The interpreter is checked against SingleStepTests (`just sst`,
`crates/armv4t/tests/sst.rs`) and by unit tests in `interp/tests.rs`. The
builder is checked the same way through `analysis::ir::eval`, a reference
IR evaluator that runs a block on the interpreter's `Cpu` and `Bus` and
shares its banking, `msr` masking and exit handling (`Cpu::msr_cpsr`,
`Cpu::apply_exit`, ...). `just sst-ir` (`analysis/tests/sst_ir.rs`) runs
every SingleStepTests case through decode, build, DCE and the evaluator,
and compares the result with the expected state and with the interpreter's
run, bus accesses included; it prints the same table as `just sst`. It then
runs a million blocks of 2 to 6 random SingleStepTests opcodes both ways
on a hashed memory, which is what reaches the builder's forwarding across
instructions and its cache flushes. Unit tests in `analysis/src/ir/tests.rs`
check the printed shape of a few blocks.

## The trait

All methods take `&mut self` (the builder appends on every call, even for
pure ops). The trait is `Sized` and not object-safe (`when` is generic);
everything is monomorphised.

**Values.** `V` and `B` are separate associated types, so generic semantic
code can't mix a flag with a word: rustc rejects it even though the builder
uses `ValueId` for both. Flags and conditions are `B`; `bit_to_word` and
`test_bit` convert.

**Pure ops.** `imm`, `bit`, `add`, `sub`, `and`, `or`, `xor`, `not`, `mul`,
`mul_long`, `mul_carry`, `add_with_carry`, `shift`, `rrx`, `sext`, `select`,
`bit_to_word`, `test_bit`, `is_zero`, `bnot`, `band`, `bor`, `bxor`. Their
exact `u32` meaning is in `sem::prim` where it isn't a Rust operator; the
interpreter calls those functions, and generated code will too (through the
runtime), so the interpreter and recompiled code can't disagree.

- `add_with_carry(a, b, c) -> (result, carry, overflow)` is the ARM ARM's
  `AddWithCarry`. Every flag-setting add, subtract and compare goes through
  it (`a - b` is `add_with_carry(a, not b, 1)`, as in the ARM ARM), so the
  carry and overflow rules exist in one place.
- `shift(kind, v, amount, carry_in) -> (result, carry)` is the barrel
  shifter with a *register* amount (low byte; 0 leaves value and carry
  alone; 32 and up follow the ARM ARM). Immediate shifts are mapped onto it
  by the semantic helpers, which handle the odd `#0` encodings (`lsr #0` is
  `lsr #32`, `ror #0` is `rrx`) statically, as plain Rust. The constant
  amount then lets codegen emit a plain `<<`.
- The set is meant to cover the whole ARMv4T ISA. The 64-bit multiplies use
  `mul_long` plus an `add_with_carry` chain for the accumulate.

**CPU state.**

- `reg` / `set_reg` take r0 to r14 of the *current mode's bank*. Banking is
  the implementer's business. They never see r15 (below).
- `user_reg` / `set_user_reg` are the user bank, for `ldm`/`stm` with `^`.
- `flag` / `set_flag` take one of N, Z, C, V. Flags are four separate pieces
  of state, not bits of a CPSR word, so the IR has one `SetFlag` per flag
  and flag liveness can treat each one alone.
- `cpsr` / `set_cpsr(v, mask)` and `spsr` / `set_spsr(v, mask)` are `mrs` /
  `msr`. `mask` is the encoding's field mask (control, extension, status,
  flags). A control-byte write can switch mode; the implementer rebanks.
  `cpsr()` includes the flags, so it reads all four.

**Memory.** `load(width, addr)` and `store(width, addr, v)` are raw bus
accesses: the bus aligns the address down to the width, loads zero-extend,
stores truncate. The ARM7TDMI's quirks (a misaligned `ldr` rotates the word,
a misaligned `ldrsh` loads a sign-extended byte) are written in the semantic
functions with pure ops (see `arm::load_word_rotated`), so the bus stays
simple, and when the address is a known constant const-prop folds the
rotation away.

**Control.**

- `when(c, f)` runs `f` only if `c`. It's for condition codes: the helper
  `conditional(s, cond, f)` evaluates the condition and skips `when`
  entirely for AL, so unconditional instructions build no guard. `f` can't
  return a value, so whatever happens under a condition reaches the outside
  only through effects, and the builder can merge it.
- Exits end the instruction:
  - `jump(target)`: stay in the current instruction set. `b`, `bl`, any
    write to pc (`mov pc, ..`, `ldr pc, ..`, `pop {pc}`, which don't
    interwork on ARMv4). The implementer decides what happens to the low
    bits: the interpreter keeps both in ARM state and clears bit 0 in Thumb,
    as NanoBoyAdvance (and so SingleStepTests) does; only fetches align.
  - `jump_exchange(target)`: `bx`. Bit 0 picks Thumb.
  - `exception_return(target)`: CPSR = SPSR, then continue at `target`
    (the interpreter doesn't align it; fetches align). `movs pc, lr`,
    `subs pc, lr, #4`, `ldm ..^` with pc. One op, because the copy and the
    jump always come together.
  - `swi(comment)`: the implementer either enters supervisor mode at 0x08 or
    runs the BIOS call in Rust (decision 12) and resumes after the `swi`.
  - `undefined()`.

There is no "call" op. `bl` is `set_reg(lr, ..)` then `jump(..)`; decision 8
recognises calls by that pattern (and by `mov lr, pc; bx rN`), so the
semantics don't have to guess intent.

### pc is static

Semantic functions get `at`, the instruction's own address, as a `u32`, and
read pc through `read_reg(s, r, at, pc_offset)`, which turns r15 into
`s.imm(at + pc_offset)`:

- ARM: `at + 8` for most operands;
- ARM: `at + 12` ("late") for rm and rn of an instruction that shifts by a
  register (rs is read a cycle earlier, at `at + 8`), for a stored pc
  (`str pc`, `stm` with pc), and for every operand of `swp` and the
  multiplies;
- Thumb: `at + 4` (and `(at + 4) & !2` for pc-relative `ldr` and `add`).

`execute` passes the instruction set's pc base (8 in ARM, 4 in Thumb) to
every semantic function; "late" is one instruction further (12, or 6 for
the pc an empty Thumb `stmia` stores).

Writes go through `write_reg`, which turns r15 into `jump`. So `Sem::reg`
and `Sem::set_reg` never see pc, and in the IR every pc read is a constant.
That's what makes literal pools fall out of const-prop: `ldr r0, [pc, #4]`
loads from `const + const`.

## Rules for implementers

This is what the interpreter (#4) needs.

**Who owns CPU state.** The implementer. The trait says nothing about
layout. A natural interpreter is a short-lived struct per instruction
holding `&mut Cpu` (registers with banks, flags, CPSR control bits, SPSRs,
the current pc and T bit) and `&mut Bus`, plus the instruction's address
and an `Option` for the exit taken.

**How pc is read.** The semantics never ask the implementer for pc: they get
`at` as an argument and add the pipeline offset themselves. The interpreter
keeps pc as "the address of the next instruction to execute" and passes it
as `at`. If a test format stores the pipelined value (SingleStepTests store
r15 as seen by the pipeline), convert at the harness boundary.

**How effects are sequenced.** In call order, applied immediately: a read
sees every earlier write by the same instruction. Semantic functions read
their operands before writing anything unless the hardware orders it
otherwise (an `ldr` whose base is also its destination: the load wins, so
the semantics skip the writeback). After an exit on a path, no more effects
happen on that path. The interpreter (in debug builds) and the IR builder
assert this.

**Exits.** An exit records where to go next; it doesn't run anything. If an
instruction finishes with no exit, the interpreter loop advances pc by the
instruction size. Roughly:

`Cpu::execute_decoded` in `interp.rs` is the reference: it runs
`sem::arm::execute`, then applies the exit (or advances pc).

**`when`.** For `V = u32`: `if c { f(self) }`.

**`at` lives in two places.** Semantic functions get it as an argument, but
`swi` and `undefined` take none, and an implementer that enters the
exception (lr = the next instruction) needs it. So the implementer keeps its
own copy of the current instruction's address too: the interpreter already
has it, and the builder records it in `begin_insn`.

**The primitives.** Call `sem::prim` for `add_with_carry`, `shift`, `rrx`,
`sext`, `mul_long` and `mul_carry` rather than re-deriving them.

## Settled against SingleStepTests (#4)

SingleStepTests ARM7TDMI was generated with NanoBoyAdvance, so where ARMv4
says "unpredictable", the interpreter follows NBA. `just sst` prints the
per-file table; issue #4 lists the files below 100% and why.

- **Compares with rd = pc and S set** copy SPSR to CPSR after setting the
  flags, and don't branch (about 1,900 cases per data-processing file).
- **`movs pc, ..` in user or system mode** (no SPSR) sets the flags from the
  result, then jumps. So `data_processing` sets flags first and then calls
  `exception_return`, which leaves CPSR alone when there is no SPSR.
- **C after a multiply** comes from the multiplier's Booth carry-save
  adders: `Sem::mul_carry`, `prim::mul_carry` (a port of zaydlang's
  reference model), used by every flag-setting multiply.
- **pc bits on a jump**: ARM keeps both low bits, Thumb clears bit 0
  (except an empty-list `ldmia`/`pop`, which NBA leaves alone: a listed
  failure).
- **Writeback to pc** (unpredictable): a single transfer accesses with pc at
  `at + 8`, but writes back `at + 12 ± offset` and jumps there; an `ldm`
  with writeback to pc jumps to the new base after its loads.
- **`stm`/`ldm ^` with writeback** write the base back to the user bank.
- **The CPSR mode's bit 4 is always 1** on a write.
- **Registers are banked as NBA banks them**, which is also how the tests
  record them: `R` is the user-mode view and each mode's own registers are
  in its bank array.

## Decoded instructions in, Thumb as ARM

`sem::arm::execute(s, at, thumb, instr)` matches on `decode::Instr`, applies
the condition with `conditional` and calls one function per variant, taking
the decoder's own types (`Operand2`, `Shift`, `Addr`, `MemSize`, ...). `sem`
reuses the decoder's `Cond` and `ShiftType`; `decode::Reg` converts into
`sem::Reg`.

The decoder folds Thumb into the ARM variants (Thumb `lsl rd, rs` is
`Alu { op: Mov, s: true, op2: RegShiftReg }`, `neg` is `rsbs rd, rs, #0`),
so Thumb runs through the same functions with pc base 4. The `t` variants
(`ldrt`, `strt`) run as the plain ones: the GBA has no MMU for them to
differ on. Thumb's two-halfword `bl` is `ThumbBlHi { offset }`
(lr = `at + 4 + offset`) and `ThumbBlLo { offset }` (jump to `lr + offset`,
then lr = `(at + 2) | 1`), each `set_reg` plus `jump`.

`armv4t` is `no_std` (unarm needs nothing beyond `core`), and
`just check-nostd` builds it, so the interpreter can live in the `no_std`
runtime (decision 6).

## The IR

A `Block` is one guest *superblock*: one entry, a straight line of insts,
guarded side exits, and a final unguarded exit. Each `Inst` is an `Op` plus
an optional guard bit, and defines the SSA value named by its index.
Operands always point backwards. Insts run in order; a guarded inst runs
only if its guard is set; the first exit that runs leaves the block. A
skipped inst's value (a guarded `Load`) is unspecified; the builder only
uses it under the same guard or through a `select` on that guard.

Ops fall into groups that passes care about:

- **pure** (`Const`, `Bin`, `Adc`, `AdcCarry`, `Shift`, `ShiftCarry`,
  `Select`, `TestBit`, …): depend only on their operands; can be moved,
  folded, dropped when unused.
- **reads** (`GetReg`, `GetFlag`, `GetCpsr`, …): see the CPU state at their
  position, so they don't move past writes; dropped when unused.
- **effects** (`SetReg`, `SetFlag`, `SetCpsr`, `Load`, `Store`) and
  **exits**: always kept, in order. `Load` is an effect, not a read: I/O
  reads can have side effects and must stay ordered against stores.
  Const-prop may later prove a load from ROM pure.
- `Insn(addr)` marks each guest instruction's start, for comments in the
  output and for mapping back to guest addresses.

Ops with several results in the trait (`add_with_carry`, `shift`) become
several single-result insts (`Adc`, `AdcCarry`, `AdcOverflow`). A dead
carry is then an unused value, and plain DCE removes it, without
projections or tuple types.

**Conditional execution is predication.** `moveq r4, #1` becomes
`if v10: set r4, v17`. A conditional branch becomes a guarded exit, and the
block carries on after it. The builder never splits a block for a
condition code.

**The builder forwards values.** It caches the SSA value of each register
and flag. A read after a write in the same block reuses the written value
instead of emitting a `Get*`, so `cmp` followed by `beq` uses the `is_zero`
value directly, and const-prop sees values flow from instruction to
instruction. A guarded write caches `select(guard, new, old)`. Every write
is still emitted, in order; forwarding only saves reads. The cache is
dropped when state changes behind the builder's back: a control-byte CPSR
write (mode switch rebanks r8 to r14), a flags-byte CPSR write, and any
user-bank write.

A control-byte CPSR write doesn't end the block. Registers live in
`cpu.r[]` under the current bank, so a mode switch mid-block is handled by
the runtime, and an IRQ unmasked mid-block is taken at the next sync point
(decision 11), which is a few instructions late at most.

**Sync points inside a block.** Decision 11 syncs after I/O writes, which
can fall mid-block, and an IRQ taken there runs guest code before the block
continues. The rule: at a sync point, state that is *live afterwards* must
be in `cpu`. In safe mode (decision 10) it is, because every write is
emitted in order. Dead state needn't be: the IRQ path saves the CPSR to
SPSR_irq and the registers it uses, and restores them on return, and
nothing on it reads the interrupted code's dead values. So deleting a dead
`SetFlag` before a sync point is safe, and the IR needs no sync marker for
liveness. It will need one for ABI-trust mode (registers held in Rust
locals must be written back there); that pass can insert a `Sync` inst.

`Block::dce` removes unused pure ops and reads in one backward pass. A
sample block, after DCE:

```text
block 0x08000000 arm
  ; 0x08000000                      adds r0, r1, r2, lsl r3
  v1 = get r3
  v2 = get r2
  v3 = lsl v2, v1
  v4 = get r1
  v5 = bit false
  v6 = adc v4, v3, v5
  v7 = adc.carry v4, v3, v5
  v8 = adc.overflow v4, v3, v5
  v9 = test_bit v6, 31
  v10 = is_zero v6
  set N, v9
  set Z, v10
  set C, v7
  set V, v8
  set r0, v6
  ; 0x08000004                      moveq r4, #1
  v17 = const 0x1
  if v10: set r4, v17
  ; 0x08000008                      ldr r5, [pc, #4]
  v20 = const 0x4
  v21 = const 0x8000010
  v22 = add v21, v20
  v23 = load.word v22
  v24 = const 0x3
  v25 = and v22, v24
  v26 = lsl v25, v24
  v27 = ror v23, v26
  set r5, v27
  ; 0x0800000c                      bx lr
  v30 = get lr
  exit jump_exchange v30
```

The shifter's carry and the old C flag were built and then removed (`adds`
takes C from the addition); `moveq` reads Z straight from `v10`; the
literal's address is `const + const`, waiting for const-prop.

## What flag liveness and const-prop need (decision 9)

**Flag liveness** (backward, per flag):

- Within a block, walk the insts backwards. An unguarded `SetFlag(f)` kills
  `f`; a guarded one doesn't (on the false path the old value survives);
  `GetFlag(f)` makes `f` live. `GetCpsr` reads all four flags, and
  `SetCpsr` with the flags mask writes all four.
- At an exit, the live-out set is whatever the successor needs. Without a
  CFG, assume all four. With one, each block gives two sets (flags read
  before written, and flags written unguarded), and a standard fixpoint
  over the function's CFG solves it. A `swi` exit or a dynamic jump keeps
  everything live.
- Mid-block sync points don't change this (see "Sync points inside a
  block" above).
- A `SetFlag` whose flag is dead is deleted, and DCE then removes the
  `AdcCarry`/`ShiftCarry`/`TestBit` that fed it. That's the whole point of
  splitting multi-result ops and keeping flags separate.

**Const-prop** (forward):

- Fold pure ops with constant operands, `select` and guards with constant
  conditions. A guard that folds to true is dropped. One that folds to
  false deletes its inst once the `select`s on that guard have been folded
  (they were the only users of its value).
- pc reads are already constants, so pc-relative addresses fold. A `Load`
  from a constant address in ROM becomes the ROM's value (needs the ROM
  image passed to the pass): literal pools disappear.
- A `Store` to a constant address in I/O space can be printed as a direct
  call to that register's handler, skipping the memory-map dispatch.
- A constant jump target is a static CFG edge; a constant `jump_exchange`
  target also fixes the instruction set of the successor (decision 5b).
- Across blocks, the inputs are the block's `GetReg`s that come before any
  cache flush (a mode switch or user-bank write); those read entry values,
  and a CFG-level pass maps known entry values onto them. A `GetReg` after
  a flush reads a different bank, so it's unknown. Within a block the
  builder's forwarding has already done the work.

Neither pass needs anything the IR doesn't have now. ABI-trust mode
(decision 10) will add a `Sync` inst for register writeback.

**As built** (`analysis::ir::flags`, `analysis::ir::constprop`, wired in
`codegen::func`):

- Flag liveness follows only a function's `Plan::Static` edges to its own
  blocks; every other exit (calls, `Rekey`, dynamic jumps, `swi`, leaving
  the function) keeps all four live. An IWRAM copy keeps all four live at
  every exit: a block whose guard fails runs in the interpreter, and a
  variant may run instead of the block analysed.
- `GetSpsr` counts as reading all four flags: with no SPSR (user and
  system mode) it reads the CPSR.
- A `Load` from ROM stays even when its value folds: the runtime counts its
  wait states (`Bus::load` → `count_data`), which the frame's timing
  depends on. Its users read a `Const` after it. Regions `0x0D` (EEPROM)
  and the GPIO port (`0x080000C4`–`C9`) never fold.
- An exit whose guard folds to true ends the block; insts under a guard
  that folds to false are deleted.
- Const-prop changes control flow, not only printed code: a jump target
  read from a ROM literal is now a constant, so `ldr pc, =X` plans as
  `Static` or `Call` (it was `Dynamic`), and `ldr rN, =X; bx rN` has a known
  target whose blocks join the function.
- A flag left stale is never seen: each exit's yield path writes the flags
  whose writes liveness deleted, and a flag stays live on an edge into a
  block that doesn't write it before its first exit (step 2 of
  `docs/structured-codegen-design.md`). Every yield, a frame's end
  included, so shows the interpreter's CPSR. Until then (2026-10-24) a run
  stopped on such an edge showed a stale flags byte in `--regs`.
- Not done yet: entry values across blocks (the `GetReg` rule above),
  stores to constant I/O addresses as direct handler calls, and a
  timing-only op to replace a folded ROM load's call.

## The abstract interpretation (#17)

`analysis::vsa` runs the same semantics with `S::V` an index into a
per-instruction arena of `AbsVal`s, since a value set isn't `Copy`. The
trait needed no change. Notes for later readers:

- A flag is a range predicate over a named unknown (`Bit::Pred`), so
  `cmp r0, #7` followed by `bhi` narrows r0 with no knowledge of `cmp`:
  `add_with_carry(r0, !7, 1)` gives C as "r0 ≥ 7" and Z as "r0 = 7", and
  `hi` is `C & !Z`.
- Values keep `sym + off`, so registers related by a constant narrow
  together, and `op(sym + off)` for a shift or multiply by a constant, so an
  index scaled before its check narrows too.
- `when` splits the state on the condition and joins after; exits come back
  with the state they left with.
- Entry sp is assumed word-aligned, which `load_word_rotated`'s `addr & 3`
  needs to fold for stack slots.

## Not covered yet

- Cycle counts (decision 11): estimated per block from the decoded
  instructions, so no `Sem` op is planned. If accurate counts turn out to
  need the data (memory region of each access), the builder can record
  them per `Load`/`Store`.

## Approved by june (2026-10-19, PR #11)

june approved this design and took all three picks offered to her:

1. **Conditional instructions are predicated**: guards on straight-line
   superblocks, not IR basic blocks.
2. **ARM-shaped primitives**: `add_with_carry` and the barrel `shift`, not
   tiny compare/select ops.
3. **`swi` ends the block** (`Exit::Swi { comment, next }`), not an inline
   call effect.

## Choices and the alternatives rejected

- **pc as a static argument** rather than `reg(PC)` with the implementer
  adding the offset: the offset depends on the instruction form (8 or 12),
  which only the semantics know, and a static pc makes every pc read a
  constant in the IR.
- **Separate `B` type** rather than flags as `u32` 0/1: rustc catches
  mixing them, and the printer can emit `bool`.
- **Flags as four pieces of state** rather than a CPSR word: per-flag
  liveness needs per-flag writes.
- **`AddWithCarry` and an ARM-style `shift` as primitives** rather than
  small ops (compare, select) composed in each semantic function: one
  definition of each rule, short IR, and const-prop still folds them when
  the operands are constant. Cost: the codegen prints them as helper calls
  when the operands aren't known.
- **Raw bus loads** rather than loads with ARM7TDMI rotation and sign
  extension built in: the bus implementation (runtime, abstract
  interpreter) stays simple, and the quirk is written once in the
  semantics.
- **Predicated superblocks** rather than splitting conditional instructions
  into IR basic blocks (a CFG inside each block): the block stays one
  straight line, so the backward and forward passes are single loops, and
  the printer can turn guards back into `if`s. Cost: a long run of
  instructions under the same condition repeats its guard on every effect.
- **`when` with a closure that returns nothing** rather than an
  if-then-else that merges values (phi-style): merging happens only through
  state, which the builder already tracks.
- **Split single-result IR ops** rather than tuple results with
  projections: dead flags fall out of ordinary DCE.
- **Separate exit methods** rather than one `exit(Exit)` method on the
  trait: each has different static arguments, and the trait reads as a list
  of what the CPU can do.
- **Generic methods, no `dyn Sem`**: zero-cost interpreter, and `when` can
  take a closure.
- **`V = TokenStream`** was the original plan; decision 3 already rejected
  it because backward analyses need an IR.
