# Risks to keep an eye on

Things that could force a design change. Check them at each milestone; each
says what to measure and what we'd do.

## Rust backend (decision 2)

- **No `goto`.** Reducible CFGs (nearly all compiler output) translate to
  structured Rust with labeled blocks, `loop`, `break 'label` and `continue`,
  the same shape as WebAssembly's block/loop/br. Prior art: Ramsey, "Beyond
  Relooper" (ICFP 2022, functional pearl); Binaryen/LLVM's Wasm stackifier;
  Emscripten's Relooper (Zakai 2011); decompiler structuring (Cifuentes 1994;
  Yakdan et al., "No More Gotos", NDSS 2015; Erosa & Hendren 1994).
  Irreducible regions (hand-written asm) fall back to `loop { match block }`
  or node splitting. Measure: share of functions with irreducible CFGs.
- **Loop-match not flattened.** LLVM's jump threading usually turns the
  state machine back into direct jumps. Nightly `#[loop_match]` targets this.
  Measure: dispatch overhead in a profile of a state-machine-heavy function.
- **No guaranteed tail calls.** `become` is nightly-only. Guest tail calls
  (`b other_fn`) nest Rust calls; decision 8's unwinding to a dispatcher
  bounds the stack. Measure: max host stack depth in a long play session.
  Since the guest runs on its own 1 MiB stack (structured-codegen-design
  step 7), every route writes at most 10 KB of it (Rogue, 2026-10-25).
- **Compile time.** rustc/LLVM are slow on huge functions and huge match
  statements. Split the output into many modules/crates. Measure: clean
  build time per game; worry above ~10 minutes.

If one of these can't be solved, the fallback backend is LLVM IR; what that
would look like and cost: [research/llvm-backend.md](research/llvm-backend.md).
