# Async codegen and Rust coroutines: why not

Two spikes run on 2026-10-24 on quack, while june decided how structured
code handles resumes ([structured-codegen-design.md](../structured-codegen-design.md),
steps 5–8). Both asked whether the guest's call chain could be *suspended*
in place at an IRQ or the frame end, rather than unwound, by having rustc
turn each guest function into a state machine. The answer was yes, but at
costs the stackful alternative (the guest on its own stack, step 7)
doesn't pay, so june chose that. This file keeps what was measured, so
nobody has to measure it again. The spike code was deleted; it was
throwaway.

## Async codegen

**The idea.** Every guest function is an `async fn`, every `rt::tick` a
possible `.await` (Pending when an IRQ or the frame end is due), and every
known call `f_x(..).await`. An executor of our own runs the chain. At a
stop the whole chain of futures suspends and later continues exactly
where it was, so nothing unwinds.

**Compile cost (measured).** Real p07 (19,880 instructions), converted
by a throwaway Rust converter that kept v1's `loop { match pc }` shape, so
the only difference rustc saw was the coroutine lowering. Ticks became
`if rt::tick(m, c) { park(mp, pc).await }`, calls `f(mp, K).await`, and
each function got a type-erased starter for the table and other parts:
3,151 suspend points and 1,208 awaited calls, 980 of them into other
parts. Same scratch workspace, release profile, `P = Headless`, under the
build lock:

| p07 | v1 | async |
|---|---|---|
| release wall time | 26.0 s (3 runs) | 26.7 / 27.4 / 32.7 s |
| release peak RSS | 710–747 MB | 1.15–1.16 GB (+55%) |
| `cargo check` (front end) | 4.6 s, 447 MB | 7.3 s, 834 MB |
| rmeta / rlib | 0.33 MB / 3.4 MB | 21 MB / 26 MB |

The metadata is 60× because async bodies' MIR is encoded in it. Only
about 228 of p07's awaits nested a callee's future inline (the rest were
type-erased), so deep inline nesting was under-tested at this size.

**Future sizes and the call graph (measured).** Futures were 40–200 bytes,
median 72 (`-Zprint-type-sizes`), with `mp` and `pc` each stored twice
(rust-lang/rust#62958). A census of Emerald's generated code:

- 15,167 functions, 153,423 ticks;
- 53,500 direct call sites, **36,848 (69%) crossing part crates**;
- 553 indirect calls, 153 tail jumps, 16,437 computed exits;
- recursion: 3 self-recursive functions and 2 SCCs (of 3 and 2), so only
  21 call sites would need a box;
- the deepest static call chain is 20 functions, so even fully inlined
  the root future would be 1–2 KB.

The 69% is the structural problem: a caller's future must name its
callee's future type, and an `extern "Rust"` declaration across part
crates can't. So those calls become type-erased frames. Partitioning parts
in topological order (the graph is almost acyclic) would allow naming, but
then the parts build one after another, each reading the others' 20 MB of
metadata. That was not measured.

**Runtime (toy microbenchmark, measured).** 1M iterations, events every
1,232 cycles:

- the tick's fast path compiles to v1's branch: 1.00–1.10× v1;
- a callee rustc inlines is free (0.7–0.9×);
- a type-erased call through a LIFO frame arena: 1.4–1.8× v1's loop;
  through `Box::pin`: 2.3–2.9×;
- a callee whose future constructor isn't inlined: 4–4.5×, about 12 ns a
  call (from the assembly, moving the callee's future into the caller's
  state stalls store forwarding).

**Soundness (Miri, Stacked and Tree Borrows).** Futures hold only
`Mp(*mut M)`. Generated code re-derives `let m = mp.get()` at entry and
after every `.await` and never keeps a `&mut M` across one. The frontend
derives its own `&mut` from the same raw pointer, only between polls. That
passes; a variant holding `m` across one await fails under both models.
The runtime's own `park` re-deriving `m` was enough to cause the conflict,
so the rule is needed even with no frontend involved.

**`no_std` (built).** A `#![no_std]` toy with no alloc built for
`riscv32imac-unknown-none-elf`. Type-erased frames (indirect calls,
recursion, IRQ handlers, other parts) lived in a fixed LIFO arena, sound
because a callee's frame is always freed before its caller's, even when a
whole chain is dropped. High-water mark: 944 bytes.

**Semantics.** IRQ: taken in `park` at the tick's key and run as a nested
frame. Halt: `idle()` inside `park`. Frame end: Pending to the frontend;
`park` must write `cpu.pc` (and with locals, the write-backs) so a save
state there is valid. A run suspended at every frame end had the same
event hash, clock and IRQ count as one never suspended. But resumes come
back: a state load or rewind drops the chain, and every caller still on
the guest stack then resumes in the interpreter; a guest returning
somewhere other than its caller returns that key up the chain, also
interpreted.

**Why not.** +55% front-end memory and 60× metadata on every part, the
thing phase 1 fought; most calls slowed by type erasure across parts;
`.await` noise on every tick and call; and it still needs the same wide
call settling as the stackful design. Its one advantage: no second stack,
so it works on any ESP32, including the Xtensa S3, which `corosensei`
doesn't support.

## Nightly coroutines with resume arguments

june's idea, after JS generators: each guest function is a coroutine that
`yield`s the `&mut Machine` out at a stop and gets one back on `resume`.
Tested on nightly 1.101 (2026-10-21):

- Resuming with a *fresh* `&mut machine` each time doesn't compile (E0503):
  a resume argument's lifetime is fixed for the coroutine type, so the
  first borrow lasts until the last resume. Higher-ranked forms
  (`impl for<'a> Coroutine<&'a mut M>`, closure lifetime binders) fail
  with E0277, and rustc's `tests/ui/coroutine/resume-arg-late-bound.rs`
  treats that as intended.
- Handing the borrow back and forth like a baton does compile, with no
  unsafe: `m = yield m;`. The driver uses M only through the yielded
  reference and resumes with it, and keeping a reborrow across a `yield`
  fails to compile. The app must reach the Machine through the baton, so
  it owns a `Box<M>` or a `&'static mut M`. Miri ran it clean.
- Nesting needs a hand-written "yield from" (about six printed lines per
  call site); callees must be `static` coroutines; recursion and calls
  through function pointers need `Pin<Box<dyn Coroutine<..>>>`, as in
  async.
- Stable async can do the same baton through a `Cell<Option<&mut M>>`
  slot with no unsafe, same output. The MIR lowering is the same
  (`{closure#0}` with a state switch), so compile cost should match async.
- Status: the `coroutines` tracking issue (#43122) needs a full RFC before
  stabilization and had no activity since January 2026; `gen_blocks`
  (#117078) has no resume arguments. A dated nightly pin would carry the
  whole build, clippy, rustfmt and the riscv target with it.

**Why not.** The same as async, plus a nightly pin, minus `.await`'s
forwarding.

## Prior art found

- [An async/await Game Boy emulator](https://sanjeetnd.com/async-await-emulators):
  one task per component, boxed futures, no numbers, says save states are
  impossible.
- [bagnalla/6502](https://github.com/bagnalla/6502): a 6502 core on nightly
  coroutines, yielding every cycle.
- SqueakJS tried JS `yield*` on every send and found it very slow
  ([vm-dev](https://lists.squeakfoundation.org/pipermail/vm-dev/2021-April/036133.html));
  jsSMS lists generators for interrupts as a to-do.
- rustc: [#83031](https://github.com/rust-lang/rust/issues/83031)
  (exponential build times with nested async), the
  [2026 goal to shrink futures](https://rust-lang.github.io/rust-project-goals/2026/async-future-memory-optimisation.html),
  [Generalized coroutines](https://lang-team.rust-lang.org/design_notes/general_coroutines.html),
  [Passing borrows through resume](https://internals.rust-lang.org/t/passing-borrows-lifetimes-up-into-a-generator-through-resume/13497).
- For the stackful side: near's
  [cooperative threading](https://archive.ares-emu.net/near.sh/articles/design/cooperative-threading.html)
  (libco in higan and ares), gbarecomp's nested IRQs, jamulator's nested
  NMI.

No static recompiler using stackless coroutines was found, and nothing
measuring rustc with thousands of await points.
