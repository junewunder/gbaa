# An LLVM IR backend: what it would look like

Written 2026-10-26 on quack, after june asked whether translating ARM
straight to LLVM IR might be better than emitting Rust. Research only:
nothing was built, and decision 2 (emit Rust, LLVM IR as the fallback)
still stands. june thinks there may be a better way than this; the spike
at the end is on hold until she says.

**What june wants from it:** the interface (UI, PPU, sound, save states,
netplay) built in Rust around an opaque, unsafe blob of compiled GBA code.

## How Rust and LLVM IR link

Rust doesn't link to "LLVM". Both sides become ordinary object files and
the linker joins them through C-ABI symbols: LLVM IR compiles (`llc`, or
inkwell's `TargetMachine::write_to_file`) to a `.o` or `.a` like any
other. Rust calls into the blob through `extern "C"` declarations, and the
blob calls back into Rust through `#[no_mangle] extern "C" fn`. Both
directions work.

```
gbaa-recompile ──> out/emerald/game.ll  (or .bc, via inkwell)
                        │  llc / inkwell TargetMachine, per target triple
                        ▼
                   libemerald.a   exports: gbaa_code (dispatch table), f_0809d301, …
                                  imports: gbaa_rt_tick_slow, gbaa_io_load, gbaa_swi, gbaa_interp_fallback …

crates/runtime + desktop (Rust)
   build.rs: cargo:rustc-link-lib=static=emerald
   extern "C" { static gbaa_code: CodeTable; }
   #[no_mangle] extern "C" fn gbaa_io_load(m: *mut Machine, addr: u32, w: u32) -> u32 { … }
   safe wrapper: Game::run_frame(&mut Machine)
```

This is already close to today's shape: each generated part crate reaches
the main crate's table through `extern "Rust" { #[link_name = "gbaa_code"]
static CODE }`. The difference would be that the parts come from our own
IR emitter, not rustc.

## The catch: inlining across the boundary

Generated code is fast because `m.bus.load(..)`, `rt::tick`, `rt::fetch`
and `prim::add_with_carry` inline into it. In a separately compiled blob
each becomes an opaque call, about one per guest load or store. Three ways
out:

1. **Emit the fast paths into the IR.** RAM and ROM accesses become
   pointer arithmetic on a `#[repr(C)]` `Machine` with fixed field
   offsets, and only I/O and slow paths call into Rust. This is what JITs
   and most recompilers do. Cost: the hot memory and timing logic lives
   twice, in Rust and in the IR emitter, and the two must agree exactly.
2. **Cross-language LTO.** Build Rust with `-Clinker-plugin-lto`, emit the
   blob as bitcode, link with lld. Needs the same LLVM major version on
   both sides (quack's stable rustc: LLVM 23.1.1). Mismatches can
   miscompile silently, and full LTO has been reported broken even with
   matching versions.
3. **Link the runtime's bitcode in by hand.** `rustc --emit=llvm-bc`, then
   `llvm-link` it into each module before optimising. Same version
   constraint.

**Genericity.** `Machine<P>` is generic over the platform; a blob can't
be. It would see one concrete `repr(C)` CPU and memory layout, and Rust
would export callbacks monomorphised for the chosen `P`.

## What it would buy, against today

- **Real `goto` and `musttail`.** These answer decision 2's listed costs,
  but structured codegen and the guest stack already work around both.
- **Per-function optimisation levels** (`-O1` for cold code, full for hot
  loops), which is hard from Rust source.
- **Compile time, less than hoped.** Part p07 (19,880 instructions) takes
  4.7 s for `cargo check` (rustc's front end) and 60 s for a release
  build (structured-codegen-design.md, "Measured"). About 90% of the time
  and most of the RAM is LLVM optimising, which an IR backend still pays.
  Skipping rustc saves the front end, not the bulk.

What it would cost:

- No type checking of the output: emitter bugs show up as segfaults or
  miscompiles, not rustc errors.
- An LLVM toolchain pinned to one version in every build (`llvm-devel`
  and `clang` aren't installed on quack).
- Xtensa ESP32 targets need Espressif's LLVM fork.
- The mod view (decision 2, two views of a ROM) still needs readable
  Rust, so the Rust printer stays either way.

## Prior art

- **Remill** (Trail of Bits) lifts machine code to LLVM bitcode. Its
  README says AArch32 isn't supported yet; Thumb support couldn't be
  confirmed. trailofbits.com/tools/remill
- **LLBT**, an academic ARM/Thumb to LLVM IR static translator that also
  tackles mixed-mode code discovery without interpretation.
  ir.lib.nycu.edu.tw/handle/11536/25095
- Most game recompilers (N64Recomp, XenonRecomp) emit C or C++ instead
  ([static-recompilers.md](static-recompilers.md)).
- Cross-language LTO: doc.rust-lang.org/rustc/linker-plugin-lto.html;
  github.com/rust-lang/rust/issues/57438; the Linux kernel's
  `CONFIG_RUSTC_CLANG_LLVM_COMPATIBLE` patch (lkml, 2026-02); a report of
  full LTO failing with matching LLVM 20.1.8,
  users.rust-lang.org/t/questions-regarding-linker-plugin-based-lto/134070

## If it's tried: the spike (on hold)

The best fit is a **second printer for the play view only**, from the
same analysis and IR, with the mod view staying Rust. To settle it
cheaply: emit p07 as `.ll` with fast paths inline (way 1 above) and slow
paths through `extern "C"`, link it into the desktop build, check the
frame hash matches, and compare build time, peak RAM and frame time with
the Rust part. If the build isn't several times cheaper, or the game runs
slower, Rust stays.
