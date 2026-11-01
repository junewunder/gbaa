# References

Papers, projects and docs to read. Add to this as decisions cite things.

## Structuring control flow (goto elimination)

For decisions 2 and 8: turning a CFG into Rust's labeled blocks, `loop`,
`break 'label` and `continue` (the same shape as WebAssembly's block/loop/br).

- Norman Ramsey, "Beyond Relooper: Recursive Translation of Unstructured
  Control Flow to Structured Control Flow" (functional pearl), ICFP 2022.
  Dominator-tree translation of reducible CFGs (Peterson, Kasami and
  Tokura 1973, in one pass). Likely what we implement.
- Alon Zakai, "Emscripten: An LLVM-to-JavaScript Compiler" (the Relooper),
  SPLASH/OOPSLA companion 2011.
- Binaryen's Relooper and LLVM's WebAssembly CFGStackify pass (the
  stackifier), in their source trees.
- Cristina Cifuentes, "Reverse Compilation Techniques", PhD thesis, QUT, 1994
  (structuring for decompilers).
- Ana Erosa and Laurie Hendren, "Taming Control Flow: A Structured Approach to
  Eliminating Goto Statements", ICCL 1994.
- Khaled Yakdan et al., "No More Gotos: Decompilation Using Pattern-Independent
  Control-Flow Structuring and Semantics-Preserving Transformations", NDSS 2015.
- Edward J. Schwartz, JongHyup Lee, Maverick Woo, David Brumley, "Native x86
  Decompilation Using Semantics-Preserving Structural Analysis and Iterative
  Control-Flow Structuring" (Phoenix), USENIX Security 2013.
- Zion Leonahenahe Basque et al., "Ahoy SAILR! There is No Need to DREAM of
  C: A Compiler-Aware Structuring Algorithm for Binary Decompilation",
  USENIX Security 2024; angr's `angr/analyses/decompiler/structuring/`
  (phoenix.py, dream.py, sailr.py): github.com/angr/angr
- Andrea Gussoni, Alessandro Di Federico, Pietro Fezzardi, Giovanni Agosta,
  "A Comb for Decompiled C Code" (rev.ng), ASIA CCS 2020; revng-c's
  `lib/RestructureCFG/`: github.com/revng/revng-c
- Ghidra's decompiler structurer and switch recovery:
  `Ghidra/Features/Decompiler/src/decompile/cpp/{blockaction,block,jumptable}.cc`
  in github.com/NationalSecurityAgency/ghidra
- Hex-Rays: docs.hex-rays.com (microcode view, decompiler configuration);
  Ilfak Guilfanov, "Decompiler Internals: Microcode", Black Hat USA 2018.
- Yuri Iozzelli, "Solving the structured control flow problem once and for
  all" (Cheerp's stackifier), Leaning Technologies, 2019; Cheerp's
  `CFGStackifier.cpp` in github.com/leaningtech/cheerp-compiler
- c2rust's relooper (C goto to Rust labeled blocks and loops):
  `c2rust-transpile/src/cfg/` in github.com/immunant/c2rust
- Rust's `#[loop_match]` (direct jumps for `loop { match state }`, nightly):
  rust-lang/rust #132306; RFC 3720.
- Cristina Cifuentes and Mike Van Emmerik, "Recovery of Jump Table Case
  Statements from Binary Code", Science of Computer Programming 40(2–3), 2001.
- Johan Janssen and Henk Corporaal, "Making Graphs Reducible with Controlled
  Node Splitting", TOPLAS 19(6), 1997.
- Sebastian Unger and Frank Mueller, "Handling Irreducible Loops: Optimized
  Node Splitting vs. DJ-Graphs", Euro-Par 2001 (TOPLAS 2002).
- Larry Carter, Jeanne Ferrante, Clark Thomborson, "Folklore Confirmed:
  Reducible Flow Graphs are Exponentially Larger", POPL 2003.
- Paul Havlak, "Nesting of Reducible and Irreducible Loops", TOPLAS 19(4), 1997.

What we take from them: `docs/research/structured-control-flow.md`.

## Static recompilers

- N64Recomp: github.com/N64Recomp/N64Recomp
- XenonRecomp (Xbox 360 PowerPC → C++): github.com/hedge-dev/XenonRecomp
- DolRecomp (GameCube/Wii → C): github.com/ExpansionPak/DolRecomp
- AnyPS5 (relinker, the inspiration): github.com/boykopovar/AnyPS5
- N64ModernRuntime (N64Recomp's runtime: OS threads as host threads):
  github.com/N64Recomp/N64ModernRuntime
- ReXGlue SDK (Xbox 360, XenonRecomp plus Xenia): github.com/rexglue/rexglue-sdk
- PS2Recomp (R5900 → C++, resumable functions, central scheduler):
  github.com/ran-j/PS2Recomp
- GBA recompilers: mstan's gbarecomp and arm-recomp-core (ARMv4T → C++,
  nested IRQs, co-simulation against mGBA): github.com/mstan/gbarecomp,
  github.com/mstan/arm-recomp-core; JRickey's gba-recomp (Rust tool that
  emits C): github.com/JRickey/gba-recomp; Asphaltian's GBARecomp (→ C#,
  flag liveness): github.com/Asphaltian/GBARecomp; the Kirby & the Amazing
  Mirror recomp: github.com/Felipe-Gomes-Amorim/kirby-amazing-mirror-recomp
- Graham Toal, "An Emulator Writer's HOWTO for Static Binary Translation",
  gtoal.com/sbt (c. 2005).
- Andrew Kelley, "Statically Recompiling NES Games into Native Executables
  with LLVM and Go" (jamulator), andrewkelley.me/post/jamulator.html, 2013.

## Binary lifters

- Remill, McSema and Anvill: github.com/lifting-bits/{remill,mcsema,anvill};
  Dinaburg and Ruef, "McSema: Static Translation of X86 Instructions to
  LLVM", REcon 2014.
- rev.ng: github.com/revng/revng; Di Federico, Payer, Agosta, "rev.ng: A
  Unified Binary Analysis Framework to Recover CFGs and Function
  Boundaries", CC 2017; Di Federico, Fezzardi, Agosta, "rev.ng: A
  Multi-Architecture Framework for Reverse Engineering and Vulnerability
  Discovery", ICCST 2018.
- RetDec: github.com/avast/retdec
- Ghidra SLEIGH/P-code for ARM: `Ghidra/Processors/ARM/data/languages/`,
  `GhidraDocs/languages/html/pcoderef.html`
- llvm-mctoll: github.com/microsoft/llvm-mctoll; Yadavalli and Smith,
  "Raising Binaries to LLVM IR with MCTOLL (WIP Paper)", LCTES 2019.
- Kim et al., "Testing Intermediate Representations for Binary Analysis",
  ASE 2017; Dasgupta et al., "Scalable Validation of Binary Lifters", PLDI
  2020.

## Dynamic translators

- Fabrice Bellard, "QEMU, a Fast and Portable Dynamic Translator", USENIX
  ATC 2005 (FREENIX); QEMU's TCG (`target/arm/`, `tcg/tcg.c`, `accel/tcg/`,
  `docs/devel/tcg-icount.rst`): gitlab.com/qemu-project/qemu
- Rosetta 2: Apple, "About the Rosetta translation environment" and WWDC
  2020 session 10686; Dougall Johnson, "Why is Rosetta 2 fast?" (2022);
  Koh M. Nakagawa, Project Champollion: github.com/FFRI/ProjectChampollion
- FEX-Emu (dead flag elimination): github.com/FEX-Emu/FEX; Box64 (deferred
  flags): github.com/ptitSeb/box64
- gpsp (GBA dynarec: Thumb flag liveness, RAM code tags):
  github.com/libretro/gpsp

What we take from the recompilers, lifters and translators:
`docs/research/static-recompilers.md`.

## Validating transformations

For structured codegen and registers as locals: how to know a
transformation stayed exact.

- Pnueli, Siegel, Singerman, "Translation Validation", TACAS 1998.
- George Necula, "Translation Validation for an Optimizing Compiler", PLDI
  2000.
- Martin Rinard, "Credible Compilation", MIT-LCS-TR-776, 1999.
- Zuck, Pnueli, Fang, Goldberg, "VOC: A Methodology for the Translation
  Validation of Optimizing Compilers", J.UCS 9(3), 2003.
- Tristan and Leroy: instruction scheduling validators (POPL 2008), lazy
  code motion (PLDI 2009), software pipelining (POPL 2010); Rideau and
  Leroy, "Validating Register Allocation and Spilling", CC 2010.
- CompCert (`backend/Allocation.v`, `Linearize.v`): github.com/AbsInt/CompCert;
  CompCert-KVX (`backend/Duplicate.v`):
  gricad-gitlab.univ-grenoble-alpes.fr/certicompil/compcert-kvx
- Stepp, Tate, Lerner, "Equality-Based Translation Validator for LLVM",
  CAV 2011; Tristan, Govereau, Morrisett, "Evaluating Value-Graph
  Translation Validation for LLVM", PLDI 2011; Kundu, Tatlock, Lerner,
  "Proving Optimizations Correct Using Parameterized Program Equivalence",
  PLDI 2009.
- Sewell, Myreen, Klein, "Translation Validation for a Verified OS
  Kernel", PLDI 2013; Song et al., constrained-equivalent DBT translation
  rules, USENIX ATC 2019.
- Lopes et al., "Provably Correct Peephole Optimizations with Alive", PLDI
  2015; Lopes, Lee, Hur, Liu, Regehr, "Alive2: Bounded Translation
  Validation for LLVM", PLDI 2021: github.com/AliveToolkit/alive2
- Sasnauskas et al., "Souper", arXiv:1711.04422; Schkufza, Sharma, Aiken,
  "Stochastic Superoptimization", ASPLOS 2013; Sharma, Schkufza, Churchill,
  Aiken, "Data-Driven Equivalence Checking", OOPSLA 2013
  (github.com/StanfordPL/stoke); Bansal and Aiken, "Binary Translation
  Using Peephole Superoptimizers", OSDI 2008.
- Dasgupta et al., "A Complete Formal Semantics of x86-64 User-Level ISA",
  PLDI 2019; Hasabnis and Sekar, "Lifting Assembly to Intermediate
  Representation", ASPLOS 2016.
- Sail and Isla: Armstrong et al., POPL 2019 and CAV 2021; HOL4's L3 ARM
  model with ARMv4T (`examples/l3-machine-code/arm/`):
  github.com/HOL-Theorem-Prover/HOL; Fox and Myreen, ITP 2010.
- Barthe, Crespo, Kunz, "Relational Verification Using Product Programs",
  FM 2011; Godlin and Strichman, "Regression Verification", DAC 2009;
  Sayedoff and Strichman, arXiv:2207.14364 (regression verification today).
- McKeeman, "Differential Testing for Software", DTJ 10(1), 1998; Yang,
  Chen, Eide, Regehr (Csmith), PLDI 2011; Le, Afshari, Su (EMI), PLDI 2014.
- Martignoni et al., "Testing CPU Emulators", ISSTA 2009, and "Testing
  System Virtual Machines", ISSTA 2010; Paleari et al., "A fistful of
  red-pills", WOOT 2009; Martignoni et al., "Path-Exploration Lifting"
  (PokeEMU), ASPLOS 2012.
- risu (random instruction sequences, master/apprentice):
  git.linaro.org/people/peter.maydell/risu.git (read in the fork
  github.com/rth7680/risu); KEmuFuzzer: github.com/oliver-wm/kemufuzzer
- Liu and Wang, "How Far We Have Come: Testing Decompilation Correctness
  of C Decompilers", ISSTA 2020; Zeller and Hildebrandt, "Simplifying and
  Isolating Failure-Inducing Input", TSE 2002.
- gbarecomp's co-simulation oracle (`COSIM_ORACLE.md`), Box64's
  `BOX64_DYNAREC_TEST` (`src/emu/x64test.c`), FEX's InstructionCountCI,
  Dolphin's `JITDISABLE` switches (github.com/dolphin-emu/dolphin) and
  FifoCI (github.com/dolphin-emu/fifoci), DuckStation's
  `psxdiag_tests.cpp`, mGBA's CInema, remill's `tests/X86/`.

What we take from them: `docs/research/codegen-validation.md`.

## Semantics and staging

- Carette, Kiselyov, Shan, "Finally Tagless, Partially Evaluated", JFP 2009.
- Sail ISA specification language: github.com/rems-project/sail

## Machine-code analysis

- Noonan et al., "Polymorphic Type Inference for Machine Code" (Retypd),
  PLDI 2016: github.com/GrammaTech/retypd. For the long-term
  readable-output direction.
- Lee, Avgerinos, Brumley, "TIE: Principled Reverse Engineering of Types
  in Binary Programs", NDSS 2011.
- Balakrishnan, Reps, "WYSINWYX: What You See Is Not What You eXecute"
  (value-set analysis), TOPLAS 2010.
- OSPREY (Zhang et al., S&P 2021), DIRTY (Chen et al., USENIX Security
  2022), BinSub (Smith, SAS 2024), TRex (USENIX Security 2025): newer type
  recovery, none sound for behaviour.
- Mike Van Emmerik, *Static Single Assignment for Decompilation*, PhD
  thesis, U. Queensland, 2007 (preservation, parameters, recursion);
  Boomerang: github.com/BoomerangDecompiler/boomerang
- Ghidra's decompiler (prototype trials, `unaffected`/`killedbycall`
  specs, type propagation); RetDec's `param_return` and register
  localization (github.com/avast/retdec); rev.ng's `DetectABI` and
  callee-saved folding.
- Guilfanov, "Decompilers and beyond", Black Hat 2008; Hex-Rays SDK
  (`hexrays.hpp`, github.com/HexRaysSA/ida-sdk) and RECON 2018 microcode
  slides.
- van der Veen et al., TypeArmor ("A Tough Call"), S&P 2016; Cohn,
  Goodwin, Lowney, "Optimizing Alpha Executables on Windows NT with
  Spike", Digital Technical Journal 9(4).
- ARM AAPCS32 (IHI 0042, github.com/ARM-software/abi-aa) and ATPCS (SWS
  ESPC 0002 B-01, 2000); agbcc: github.com/pret/agbcc

## SSA and register promotion

- Braun, Buchwald, Hack, Leißa, Mallon, Zwinkau, "Simple and Efficient
  Construction of Static Single Assignment Form", CC 2013.
- Cytron, Ferrante, Rosen, Wegman, Zadeck, "Efficiently Computing Static
  Single Assignment Form and the Control Dependence Graph", TOPLAS 1991.
- LLVM's mem2reg and SROA (`PromoteMemoryToRegister.cpp`, `SROA.cpp`):
  github.com/llvm/llvm-project
- QEMU's TCG globals and helper flags (`tcg/tcg.c`, `include/tcg/tcg.h`).
- Dolphin's Jit64 register cache: github.com/dolphin-emu/dolphin
- Engelke, Schulz, "Instrew", VEE 2020; Engelke, Okwieka, Schulz,
  "Efficient LLVM-Based Dynamic Binary Translation", VEE 2021.
- Altinay et al., "BinRec", EuroSys 2020.
- Rocha et al., "Lasagne", PLDI 2022 (stack-region facts).

What we take on registers, calling conventions and types:
`docs/research/registers-and-types.md`.

## Hardware

- ARM7TDMI Technical Reference Manual and the ARM Architecture Reference
  Manual (ARMv4T).
- GBATEK (problemkaputt.de/gbatek.htm): the de facto GBA hardware spec.
- mGBA source: github.com/mgba-emu/mgba

## Decoders and test suites

- unarm (ARMv4T/v5TE/v6K decoder, MIT): github.com/AetiasHax/unarm
- SingleStepTests ARM7TDMI (per-instruction before/after CPU state, MIT):
  github.com/SingleStepTests/ARM7TDMI
- jsmolka gba-tests: github.com/jsmolka/gba-tests
- armwrestler (ARM/Thumb test ROM)

## Discovery prior art

- pret's gbadisasm (GBA disassembler with recursive descent and agbcc jump
  tables, used for the pokeemerald/pokeruby decomps): github.com/camthesaxman/gbadisasm
- Ghidra (ARM/Thumb analysis, switch recovery) and GBA loader extensions
- IDA Pro / Hex-Rays (ARM decompiler)

## Name recovery

For decision 2's mod view: names for functions, data and locals.

- pret's symbols branches (`.sym` per build, CI-committed):
  github.com/pret/pokeemerald, pokeruby, pokefirered, pmd-red (branch
  `symbols`); the pokeemerald ELF, `.map` and DWARF from a local build.
- Hex-Rays, "IDA F.L.I.R.T. technology in depth", "Generate FLIRT
  signatures from libraries" and "FLIRT 2.0" (docs.hex-rays.com, core/flirt);
  IDA Lumina overview (docs.hex-rays.com, add-ons/lumina). rizin's FLIRT
  reader: `librz/sign/flirt.c`, github.com/rizinorg/rizin
- Ghidra Function ID and BSim: `Ghidra/Features/FunctionID`,
  `Ghidra/Features/BSim` in github.com/NationalSecurityAgency/ghidra
- BinDiff: github.com/google/bindiff
- Diaphora: github.com/joxeankoret/diaphora
- Binary Ninja WARP and SigKit: github.com/Vector35/warp,
  github.com/Vector35/sigkit
- decomp-toolkit (GameCube/Wii signatures): github.com/encounter/decomp-toolkit;
  objdiff: github.com/encounter/objdiff; ds-decomp (DS, ARM/Thumb
  signatures): github.com/AetiasHax/ds-decomp; coddog:
  github.com/ethteck/coddog
- Hyungjoon Koo, Soyeon Park, Taesoo Kim, "A Look Back on a Function
  Identification Problem", ACSAC 2021.
- Jeremy Lacomis et al., "DIRE: A Neural Approach to Decompiled Identifier
  Naming", ASE 2019 (arXiv:1909.09029).
- Qibin Chen et al., "Augmenting Decompiler Output with Learned Variable
  Names and Types" (DIRTY), USENIX Security 2022 (arXiv:2108.06363).
- Kuntal Kumar Pal et al., "'Len or index or count, anything but v1':
  Predicting Variable Names in Decompilation Output with Transfer
  Learning" (VarBERT), IEEE S&P 2024; github.com/sefcom/VarBERT
- Danning Xie et al., "ReSym: Harnessing LLMs to Recover Variable and Data
  Structure Symbols from Stripped Binaries", CCS 2024;
  github.com/lt-asset/resym
- Xiangzhe Xu et al., "Unleashing the Power of Generative Model in
  Recovering Variable Names from Stripped Binary" (GenNm, earlier LmPa),
  NDSS 2025 (arXiv:2306.02546).
- Peiwei Hu, Ruigang Liang, Kai Chen, "DeGPT: Optimizing Decompiler Output
  with LLM", NDSS 2024.
- Luke Dramko, Claire Le Goues, Edward J. Schwartz, "Idioms: Neural
  Decompilation With Joint Code and Type Definition Prediction", arXiv:2502.04536,
  2025; github.com/squaresLab/idioms
- Jingxuan He et al., "Debin: Predicting Debug Information in Stripped
  Binaries", CCS 2018; github.com/eth-sri/debin
- Yaniv David, Uri Alon, Eran Yahav, "Neural Reverse Engineering of
  Stripped Binaries using Augmented Control Flow Graphs" (NERO), OOPSLA
  2020 (arXiv:1902.09122).
- Xin Jin, Kexin Pei, Jun Yeon Won, Zhiqiang Lin, "SymLM: Predicting
  Function Names in Stripped Binaries via Context-Sensitive
  Execution-Aware Code Embeddings", CCS 2022; github.com/OSUSecLab/SymLM
- James Patrick-Evans, Moritz Dannehl, Johannes Kinder, "XFL: Naming
  Functions in Binaries with Extreme Multi-label Learning", IEEE S&P 2023
  (arXiv:2107.13404).
- Linxi Jiang, Xin Jin, Zhiqiang Lin, "Beyond Classification: Inferring
  Function Names in Stripped Binaries via Domain Adapted LLMs" (SymGen),
  NDSS 2025.
- Guoqiang Chen et al., "ReCopilot: Reverse Engineering Copilot in Binary
  Analysis", arXiv:2505.16366, 2025.
- Jun Yeon Won, Xin Jin et al., "REBENCH: A Procedural, Fair-by-Construction
  Benchmark for LLMs on Stripped-Binary Types and Names",
  arXiv:2604.27319, 2026.
- Xiuwei Shang et al., "An Empirical Study on the Effectiveness of Large
  Language Models for Binary Code Understanding", arXiv:2504.21803, 2025.
- Zeyu Gao et al., "DecompileBench: A Comprehensive Benchmark for
  Evaluating Decompilers in Real-World Scenarios", arXiv:2505.11340, 2025.
- Yujeong Kwon et al., "SoK: AI-Augmented Binary Reversing".
- Zhou Xuan, Xiangzhe Xu et al., "Identifying Adversary Tactics and
  Techniques in Malware Binaries with an LLM Agent" (TTPDetect),
  arXiv:2602.06325, 2026.
- Not read, cited through others: NFRE (ISSTA 2021), AsmDepictor (AsiaCCS
  2023), HexT5 (ASE 2023); LLM4Decompile (arXiv:2403.05286) and Nova
  (ICLR 2025) score re-executability, not names.

What we take from them: `docs/research/name-recovery.md`.

## Asset formats and rippers

For decision 2's asset extraction: graphics and music out of a ROM at run
time.

- GBATEK, LCD sections (tiles, palettes, BG maps, OAM) and "BIOS
  Decompression Functions": problemkaputt.de/gbatek.htm. mGBA's
  `src/gba/bios.c` (HLE decompressors), `src/gba/renderers/`,
  `src/platform/qt/` (tile, sprite, map and palette viewers) and
  `src/debugger/access-logger.c`.
- pret's pokeemerald: `graphics/`, `sound/`, `src/m4a.c`, `src/m4a_1.s`,
  `include/gba/m4a_internal.h`, and its tools `gbagfx`, `mid2agb`,
  `wav2agb`, `scaninc`, `preproc`: github.com/pret/pokeemerald
- agbplay: github.com/ipatix/agbplay; midi2agb: github.com/ipatix/midi2agb
- GBA Mus Ripper (Bregalad): github.com/berg8793/gba-mus-ripper
- saptapper: github.com/loveemu/saptapper
- VGMusicStudio: github.com/Kermalis/VGMusicStudio
- Static rippers: Nintenlord.Hacking (github.com/TimoVesalainen/Nintenlord.Hacking),
  HexManiacAdvance (github.com/haven1433/HexManiacAdvance),
  PokemonGameEditor (github.com/Gamer2020/PokemonGameEditor), Tile Molester
  (github.com/toruzz/TileMolester).
- Code/data loggers: FCEUX (`web/help/CodeDataLogger.html`,
  github.com/TASEmulators/fceux), Mesen2 (github.com/SourMesen/Mesen2),
  BizHawk (github.com/TASEmulators/BizHawk).
- Texture replacement: Dolphin (`VideoCommon/TextureInfo.cpp`,
  `HiresTextures.cpp`), PPSSPP (`GPU/Common/TextureReplacer.cpp`), Azahar
  (github.com/azahar-emu/azahar), GLideN64 (`GLideNHQ/TxHiResLoader.cpp`),
  Mesen HD packs (mesen.ca/docs/hdpacks.html).
- Jim Chow, Ben Pfaff, Tal Garfinkel, Kevin Christopher, Mendel Rosenblum,
  "Understanding Data Lifetime via Whole System Simulation" (TaintBochs),
  USENIX Security 2004.
- Heng Yin, Dawn Song et al., "Panorama: Capturing System-wide Information
  Flow for Malware Detection and Analysis", CCS 2007; Dawn Song et al.,
  "BitBlaze: A New Approach to Computer Security via Binary Analysis",
  ICISS 2008.
- Andrew Henderson et al., "Make It Work, Make It Right, Make It Fast:
  Building a Platform-Neutral Whole-System Dynamic Binary Analysis
  Platform" (DECAF), ISSTA 2014.
- Vasileios Kemerlis et al., "libdft: Practical Dynamic Data Flow
  Tracking for Commodity Systems", VEE 2012.
- James Clause, Wanchun Li, Alessandro Orso, "Dytan: A Generic Dynamic
  Taint Analysis Framework", ISSTA 2007 (not read; cited through libdft).
- PANDA's taint2: github.com/panda-re/panda; Ryan Whelan, Tim Leek, David
  Kaeli, "Architecture-Independent Dynamic Information Flow Tracking", CC
  2013 (not read).
- Adam Summerville et al., "The VGLC: The Video Game Level Corpus", PCG
  2016 (arXiv:1606.07487); Matthew Guzdial, Mark Riedl, "Game Level
  Generation from Gameplay Videos", AIIDE 2016; Joseph Osborn, Adam
  Summerville, Michael Mateas, "Automatic Mapping of NES Games with
  Mappy", FDG 2017 (arXiv:1707.03908); Joseph Osborn et al., "MappyLand:
  Fast, Accurate Mapping for Console Games", AIIDE 2021.
- Aseprite file format: `docs/ase-file-specs.md` in
  github.com/aseprite/aseprite. Crates: aseprite-io
  (github.com/spebern/aseprite-io), asefile, aseprite-loader, png, midly,
  hound.

What we take from them: `docs/research/asset-extraction.md`.
