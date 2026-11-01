# Recovering names for functions and data

The literature review behind the mod view (decision 2, "Two views of a
ROM", june 2026-10-26). It is unit `mod/lit-naming`, for `mod/design`.
The mod view prints generated code with names from a source-map sidecar,
in a readable form, mostly for agents to read, on Emerald first. The
questions: where can names come from (the decomps, matching against named
code, learned models and LLM agents), what each needs as input, how each
is scored and how accurate it is, and which of them fit us.

Researched 2026-10-26 on quack. The pret material was read and measured
locally (below). The tools were read in their source, shallow-cloned at
the commits in "Sources". The papers were read as their PDFs, and the
numbers are quoted from the table named next to each. Two subagents did
the first pass over the tools and the papers; their key numbers and
source pointers were spot-checked against the PDFs and the clones. Every
source is also in [references.md](../references.md). The companion
reviews are [discovery-prior-art.md](discovery-prior-art.md) (how the
pokeemerald build and its symbols were set up) and
[registers-and-types.md](registers-and-types.md) (registers as locals,
and why recovered types are presentation only).

## Summary

- **Emerald needs no name recovery.** pokeemerald builds a byte-identical
  ROM, so its ELF names every function (15,858), every RAM global (1,057)
  and every ROM data object (23,573) by address. It also names 13,865 asm
  labels: scripts, maps and songs.
- **The ELF also has DWARF 2**, which gives each function's parameters and
  locals with a register or stack slot, struct types for every global, and
  a source file and line for every address. binutils can't read agbcc's
  DWARF, but gdb can. In a sample of 400 functions, 70% of named
  variables sit in a register. The catch: agbcc gives each variable one
  location for its whole scope, so a register name is a hint to check
  against our own liveness.
- **pret's names are stable.** In a year, 42 of 32,690 global names
  changed (0.13%), and no address moved. A sidecar keyed by address and
  pinned to a symbols-branch commit stays valid.
- **Function matching is how names reach other ROMs**, not how Emerald
  gets them. Other regions, Ruby (4,097 `sub_` names left in pret), and
  the SDK libraries every agbcc game links (m4a, libgcc, libc, the flash
  and RFU libraries) all get names this way.
  - The tools that fit Thumb code mask call offsets and literal-pool words
    and hash the rest: Ghidra's Function ID, and the decomp community's
    ds-decomp and decomp-toolkit. They take a name only when the match is
    unique or scores high, then pass names on to callees and pointed-to
    data.
  - FLIRT's 32-byte start window is weak on agbcc's uniform prologues.
  - BinDiff and Diaphora diff two whole programs; they are not libraries
    of signatures.
  - No tool publishes precision and recall on ARM.
- **Learned naming is x86-64 and Hex-Rays, and leaks.**
  - Variable names on functions not seen in training reach 35–43% exact
    match: DIRE, DIRTY, ReSym.
  - Function names on clean splits score 0.24–0.40 token F1, lower on ARM:
    SymGen, REBENCH.
  - Headline numbers are 2–3× those, because test functions also appear
    in training.
  - None of these models runs on our code as it is, and training one
    isn't worth it. What carries over is their method: give each function
    its callees' names, name bottom-up, iterate, and vote across uses.
- **An agent naming Emerald is mostly remembering.** pokeemerald is
  public, so any LLM has likely read it. The naming-sample score on Emerald
  is an upper bound, not a forecast for an unnamed game. The design should
  measure that leak, not assume it away.
- **Score with more than exact match.** Pret's own decomps share only
  28–53% of Emerald's function names, so string equality calls good names
  wrong. Use subtoken precision and recall, as NERO, SymLM and GenNm do,
  and keep exact match beside it. An LLM judge can be a third number.
- **Names change no behaviour, so the printer has a free hand.** Names,
  comments, the source file and line, the C prototype, and pret's types
  can all go in the readable form. The check that this is safe is
  mechanical: map every identifier back to its address key, and the token
  stream must equal the unnamed print.

## Names from the decomps

pret's decomps are the oracle (decision 2), so they come first. Everything
here was read or measured on quack, 2026-10-26, in the pokeemerald clone
and build that [discovery-prior-art.md](discovery-prior-art.md) set up
(`$GBAA_DATA/src/pokeemerald`, master `731ad5bf`, symbols branch
`dba968c6`).

### What pret publishes

- **The `.sym` file.** `make syms` runs `objdump -t` on the ELF, keeps
  addresses starting `02`, `03`, `08` or `09`, and cuts each line to
  `ADDR SCOPE SIZE NAME`
  (`pokeemerald@731ad5bf:Makefile:396-398`). CI builds it on every push
  to master and commits it to the `symbols` branch
  (`.github/workflows/build.yml:21-70`), so the branch is a history of
  every name pret has ever given, one commit per master commit.
  pokeemerald's has 73,204 lines: 72,132 in ROM, 709 in EWRAM, 363 in
  IWRAM; 32,085 global, 41,119 local. The flags column is gone, so a
  function can't be told from data or from an asm branch label, and the
  Thumb bit is gone too (discovery-prior-art.md, "Getting the symbol map").
- **Other symbols branches** (listed with `gh api
  repos/pret/<repo>/git/trees/symbols`, 2026-10-26): pokeruby (`78b19905`:
  Ruby and Sapphire, English rev0–2 and German rev0–1, and a German debug
  build), pokefirered (`3b401eaf`: FireRed and LeafGreen rev0, rev1 and the
  Switch release), and pmd-red (`ff61d9fc`). pokeemerald has one file;
  there is no pokeemerald-jp symbols branch.
- **How finished each is.** pokeemerald's `.sym` has no `sub_`/`nullsub_`
  placeholder names at all; pokefirered's has 2; pokeruby's has 4,097
  `sub_XXXXXXXX` names out of 50,963 lines. In Emerald's ELF, 189 of 15,608
  distinct function names contain `unk`, `unused` or `dummy`, and 5 of
  1,042 RAM object names contain `unk`. Emerald is named essentially
  everywhere; Ruby is not, and Ruby shares most of its engine with Emerald.
- **How stable the names are.** The symbols branch has 166 commits, 17 of
  them in the last year. Between `604f6d80` (the last before 2025-10-26)
  and today, of the 32,690 addresses carrying a global ROM name or a RAM
  name in both, **42 changed name** (0.13%; e.g. `MatchCall_Text_Brawly2`
  became `MatchCall_Text_Brawly_PreparingPostGame`,
  `AtkCanceller_UnableToUseMove` became `AtkCanceler_UnableToUseMove`) and
  12 addresses lost their name. Names are keyed to addresses, and the
  addresses never move because the build must match the ROM: a name file
  pinned to one symbols commit stays valid, and a newer one is a small diff.

### What a local build adds

`pokeemerald.elf` (56 MB) and `pokeemerald.map` (2.9 MB) come out of the
23-second build. They add three things the `.sym` lacks:

- **Symbol types.** `readelf -sW` gives 15,858 `FUNC` symbols (5,357
  global, 10,501 static), 23,573 `OBJECT`s in ROM (12,413 global, 11,160
  static) and 1,057 in RAM, each with a size, plus 13,865 global `NOTYPE`
  ROM labels. Those last are asm-side data: event scripts
  (`PetalburgCity_Gym_EventScript_OpenUnlockedDoors`), battle scripts
  (`BattleScript_EffectAttackUp`), map layouts, songs (`se_m_flame_wheel`).
  They name the game's bytecode, which matters once the mod view reaches
  scripts.
- **Statics collide.** 225 static names occur in more than one file
  (`sBgTemplates` 25 times, `sWindowTemplates` 21), and so do some static
  functions (`Cmd_call`, `Cmd_end`, `AddScrollArrows`). Rust has one
  namespace per module, so the printer needs either a module per source
  file or a suffix. The ELF has 893 `FILE` symbols and the line table (below)
  gives every function's file, so a module per file is cheap.
- **DWARF 2.** agbcc builds with `-g` (`Makefile:119`; line 133 is the modern build), and the ELF
  carries `.debug_info` (32 MB), `.debug_line` and `.debug_pubnames`.
  binutils 15.2's `readelf`/`objdump` reject agbcc's `.debug_abbrev` ("not
  zero terminated"), but Fedora's gdb reads it: `info line *0x080003a5`
  gives `src/main.c:90`, `ptype struct Pokemon` the full struct, `whatis
  gPlayerParty` `struct Pokemon [6]`, and `info scope` each function's
  parameters and locals with a location. Over 400 random Emerald functions
  (a gdb Python script, `info scope` on each): 109 had no parameters or
  locals; of 684 named parameters and locals, **480 are in a register**,
  43 in a stack slot (`sp`+offset), 157 "optimized out", 4 other.

So pret gives more than function names: **per-function names for
registers and stack slots, types for every global, and a source file and
line for every address**. That is ground truth for the per-function local
names the mod view's design wants, not only for functions and globals.

One caveat on the locals: agbcc's DWARF 2 gives each variable **one
location for its whole scope**, with no location lists. That is wrong
where a register is reused. `CreateBoxMon`'s sixth parameter
`fixedPersonality` arrives on the stack (`src/pokemon.c:2206`), yet DWARF
says it is in `r0` for the whole function. A local's register is a hint to
be checked against our own liveness (which register holds which value over
which address range), not a fact to print blindly.

### The linker map

`pokeemerald.map` says which object file each address range came from
(893 `.text` input sections) and why each library member was linked
(`libgcc.a(_divsi3.o)` pulled in by `src/sprite.o` for `__divsi3`). The
ELF's line table gives the same file boundaries; the map adds the archive
members, which name the agbcc libgcc and libc routines every agbcc game
links. That makes the map the cheapest source of library-function names
for other GBA games (see "Function matching" below).

### Sibling decomps don't share names

Names overlap only partly between pret's three Pokémon decomps, though
much of the engine is shared. Of Emerald's 15,608 distinct function names, 8,274 (53%) occur
anywhere in FireRed's `.sym` and 4,313 (28%) in Ruby's. Ruby's naming is
older (and 4,097 of its names are still `sub_` placeholders).
So porting names between games is
function matching on the code, never on the names; and a score that
compares a predicted name with pret's by string equality will mark a
correct-but-differently-worded name wrong even between pret's own decomps.

## Function matching

Function matching names a function from a library of already-named
functions. It needs a named build of the same or similar code: for us,
pokeemerald's ELF and objects, pokefirered's and pokeruby's, and the
library objects pret ships (agbcc's libgcc and libc, m4a, `agb_flash*`,
`librfu`, `siirtc`). Pointers are `file:line` at the commits in
"Sources".

### IDA FLIRT

- **Input.** Relocatable objects and libraries. FLAIR's `pelf`/`pcf`/`plb`
  write a `.pat` file, and `sigmake` compiles it to a `.sig`.
- **Signature.** The first 32 bytes, with the bytes that relocations
  touch marked variant. When patterns collide, a CRC16 over the bytes from
  offset 32 to the first variant byte separates them. After that come tail
  bytes, and then the names of referenced functions.
- **Ambiguity.** Colliding functions go to an `.exc` file, where a human
  keeps one name per group or drops the group. Functions under 4 bytes
  with no reference are dropped.
- **Sources.** Hex-Rays, "IDA F.L.I.R.T. technology in depth" and
  "Generate FLIRT signatures"; rizin's reimplementation:
  - the CRC16 (poly 0x8408): `librz/sign/flirt.c:203-220`;
  - the CRC check: `350-356`;
  - tail bytes, then naming every public name of the module: `379-482`;
  - referenced-function records are parsed (`635`) but never checked;
  - the `.pat` format: `librz/sign/pat.c:6-40`.
- **FLIRT 2.0 (IDA 9.5).** It records every call and data reference from
  the object's relocations, and names the targets once a caller matches.
  Hex-Rays reports correctly named functions on MSVC ARM going from 38 to
  660, with 248 left unnamed (their "FLIRT 2.0" page; a vendor number).
- **Accuracy.** Hex-Rays publishes no precision or recall, only the
  FLIRT 2.0 table above.
- **For us.** The format is byte-generic, but 32 bytes is 16 Thumb
  instructions. agbcc's prologues (`push {r4-r7, lr}`, `mov r7, r8`, …) are
  near-identical, so the CRC and the tail do most of the work. A fully
  linked ROM has no relocations, so patterns must come from pret's `.o`
  files. Untested: whether `pelf` accepts agbcc's old ARM relocations.

### Ghidra Function ID and BSim

`Ghidra/Features/FunctionID/src/main/java/ghidra/feature/fid/`.

- **Input.** A `.fidb` database filled from programs that are already
  named (`FidServiceLibraryIngest`). For us, the pokeemerald ELF would go
  straight in.
- **Hashing** (`hash/MessageDigestFidHasher.java:83-219`, FNV-1a 64). Each
  instruction is masked by Sleigh's instruction mask (`:188`), so operand
  fields drop out and registers stay.
  - The **full hash** marks only that a scalar was present.
  - The **specific hash** adds a scalar only when it has no relocation,
    isn't an address, and is under 256 in magnitude (`:147-166`).
  - Calls don't count as code units (`:214`). Functions under 4 code
    units are not hashed (`service/FidService.java:45`).
- **Extent.** Instructions only (`hash/FunctionBodyFunctionExtentGenerator.java:45`),
  so literal pools drop out and the load from the pool hashes as an
  address.
- **Scoring** (`service/FidProgramSeeker.java:314-369`).
  - Score = code units + 0.67 × matching specific constants + code units
    of matching children + code units of matching parents.
  - Parents count only when there are fewer than 500 (`:49`).
  - A short function can be "auto-passed" up to 24 units.
  - A name is applied above 14.6. If several names remain, it is applied
    only above 30 (`FidService.java:47-48`,
    `cmd/ApplyFidEntriesCommand.java:121-122`).
  - The relation hashes come from `db/RelationsTable.java:65-118`.
  - Very common callees are kept out of the relations
    (`markCommonChildReferences`).
- **Accuracy.** `ghidra_scripts/FidStatistics.java` measures false
  positives and negatives on a program whose labels are known. No
  published numbers.
- **BSim** compares decompiled p-code features instead (varnode, block and
  copy signatures, `Decompiler/src/decompile/cpp/signature.hh:179-231`)
  with TF-IDF cosine similarity (`LSHCosineVector.java:185-249`). That
  tolerates other register allocation and constants, at the cost of
  decompiling every function.

### BinDiff

- **Input.** Two BinExport files, from the IDA, Ghidra or Binary Ninja
  exporter.
- **Method.** A one-to-one diff of two programs, with no library.
  - Function matching steps: `match/call_graph.cc:371-389`. They include
    MD-index over the flow graph and the call graph, name, loops, call
    sequence, prime signature, hash and string references.
  - Basic-block steps: `match/flow_graph.cc:215-233`.
  - "Hash" is SDBM over the raw, unmasked bytes (`flow_graph.cc:291`), so
    it fails wherever addresses differ.
  - "Prime" is a product of one prime per mnemonic.
- **Scoring.**
  - Each step has a confidence: hash 1.0, prime 0.9, call-graph MD-index
    0.8, instruction count 0.4.
  - Overall confidence is a sigmoid of their mean (`differ.cc:392-411`).
  - Similarity weights edges 0.55, blocks 0.30 and instructions 0.15
    (`differ.cc:490-545`).
- **Accuracy.** No published numbers on ARM.
- **For us.** Region-to-region, or Ruby against Emerald.

### Diaphora

- **Input.** IDA with the Hex-Rays decompiler, which exports to SQLite.
  Like BinDiff, it diffs two programs.
- **Heuristics.** About 60 of them (`diaphora_heuristics.py:89-1178`), in
  three classes:
  - best: equal bytes, equal function hash, equal assembly or
    pseudo-code;
  - partial: KOKA graph hashes, rare constants, fuzzy pseudo-code hashes,
    mnemonic prime products, switch shapes, compilation-unit matches;
  - unreliable: graph shape only.
  - The byte hashes are MD5s of unmasked bytes
    (`diaphora_ida.py:2636-2642`).
- **Scoring.** A match's ratio is the best of several `difflib` ratios
  over cleaned assembly and pseudo-code, capped at 0.99
  (`diaphora.py:1614-1745`).
  - 1.0 is best; 0.5 or more is partial (0.3 for trusted heuristics);
    below that, down to each heuristic's own floor, is unreliable
    (`diaphora_config.py:137,141`, `diaphora.py:1889-1909`).
- **For us.** It needs IDA, which we don't use.

### Others

- **Binary Ninja WARP** (`warp/README.md:25-124`).
  - A block's ID is a UUIDv5 of its bytes, with relocatable instructions
    zeroed (found by analysis, not by relocations) and NOPs dropped.
  - A function's ID is the UUIDv5 of its sorted block IDs, so it is an
    exact match.
  - Ties are broken by the IDs of callers, callees and neighbours.
  - SigKit, its FLIRT-like predecessor, needs relocations, which the WARP
    README says fails on firmware.
- **IDA Lumina** keys server-side metadata by an MD5 of each function and
  skips functions under 32 bytes. Which bytes it masks is not documented.
- **decomp-toolkit** (GameCube and Wii, PowerPC).
  - A signature is (word, mask) pairs. Absolute relocations are zeroed,
    and field relocations are masked by field
    (`src/util/signatures.rs:335-357`).
  - A word matches if `(word & mask) == ins` (`:50-70`).
  - A signature only confirms a function already found by name or by
    control flow from `__start` (`src/analysis/signatures.rs:206-218`).
    It then names the relocation targets (`util/signatures.rs:150-205`).
  - It ships 140 SDK signature files. This is the closest design to what
    a GBA SDK library would need.
- **ds-decomp** (Nintendo DS, ARM and Thumb) is the closest to our code.
  - Literal-pool words are masked whole. Thumb `bl` pairs are masked with
    `0x07ff07ff` and ARM `bl` with `0x00ffffff`
    (`cli/src/analysis/signature.rs:83-105`).
  - The size must match exactly (`:353-362`).
  - A signature is applied only when it matches exactly one function
    (`:242-256`), and callees are named through it.
- **objdiff.**
  - It runs a patience diff over opcodes, then scores penalties: 100 for
    an insertion or deletion, 60 for a replacement, 5 for a register, 1
    for an immediate (`objdiff-core/src/diff/code.rs:56-59`).
  - match% = 1 − penalty/(100·n).
  - It decodes ARMv4T and Thumb with `unarm`, the decoder our `armv4t`
    crate already compares against.
  - `find_similar_code_symbols` ranks candidates (`diff/mod.rs:534-566`).
  - This is the right tool for *near* matches (a function edited between
    Ruby and Emerald), which exact hashes miss.
- **coddog** (ethteck) has a GBA Thumb mode, with exact, opcode-only and
  "equivalent" hashes, and bounded edit distance over opcodes
  (`crates/core/src/arch.rs:125-160,394-397`, `lib.rs:272-297`).
- **Not found.** No pret or GBA-community tool that ports names from one
  ROM to another.
- **Accuracy.** Koo et al., "A Look Back on a Function Identification
  Problem" (ACSAC 2021), finds such tools rarely benchmarked well, and
  none of the tools above has independent numbers.

## Learned naming

All of these learn from source that has debug info, compiled and then
decompiled. Almost all of it is x86-64 through Hex-Rays (IDA). Two splits
matter in every result:

- functions whose body also appears in training ("in train") score 2–3×
  higher than unseen ones;
- several datasets were later shown to leak duplicates across the split.

### Variables

- **DIRE** (Lacomis et al., ASE 2019).
  - Input: Hex-Rays tokens and AST from stripped x86-64. The truth comes
    from DWARF, aligned by which instruction offsets touch each variable.
  - Data: 164,632 GitHub binaries, 1.26 M functions.
  - Scored by exact match: 74.3% overall, **85.5% on bodies in train,
    35.3% on bodies not in train** (Table I).
- **DIRTY** (Chen et al., USENIX Security 2022).
  - A Transformer that names and types variables, on Hex-Rays x86-64.
  - On its DIRT set, 65.5% of test bodies are also in training (Table
    11).
  - Exact-match names (Table 4): **66.4 overall / 87.1 in train / 36.9
    not**. On DIRE's data: 81.4 / 92.6 / 42.8.
  - Types: 75.8% (Table 1).
  - VarBERT later found DIRT not fully stripped, and 56.9% duplicated.
- **VarBERT** (Pal et al., IEEE S&P 2024).
  - BERT, pre-trained on 5.2 M deduplicated source functions and
    fine-tuned on VarCorpus: 112,488 Gentoo x86-64 binaries at O0–O3,
    through IDA 7.6 and Ghidra 10.4.
  - It also predicts whether a variable is the developer's or the
    decompiler's own.
  - Top-1 exact match, function split vs binary split (Table 2): IDA O2
    54.4 vs 42.4, Ghidra O2 54.5 vs 40.5.
  - On IDA O0 it beats DIRE (35.9) and DIRTY (38.0) with 50.7.
- **ReSym** (Xie et al., CCS 2024).
  - Two fine-tuned StarCoder-3B models, one for local variables and one
    for struct field accesses, on IDA output of stripped binaries.
  - A Prolog step aggregates their guesses across functions into
    structs.
  - Data: 16,217 binaries, keeping only those with at least 80% unique
    functions, split by project.
  - Exact-match variable names (Table 2): **56.4 overall / 88.2 in train
    / 37.5 not**. DIRTY retrained on the same data: 48.7.
  - Field names: 16.3% (Table 5).
- **GenNm** (Xu et al., NDSS 2025; earlier arXiv versions were called
  LmPa).
  - Fine-tuned CodeGemma-2B and CodeLlama-7B/34B on IDA output.
  - Callers' and callees' already-predicted signatures go into each
    prompt, inference is repeated, and a dataflow-consistency vote picks
    among the candidate names. Preference optimisation ("SymPO") biases it
    toward developers' names.
  - Scored by subtoken precision with SymLM's synonym matcher, and by a
    GPT-4 judge.
  - Precision (Table I): 30.5 for projects not in train, 41.7 for
    projects in train on DIRTY's data; 41.2 overall on VarCorpus (VarBERT
    29.8).
  - Zero-shot GPT-4 gets 30.3, 3-shot 31.4 (Table III).
  - Its appendix shows exact-string deduplication still leaks near
    duplicates.
- **DeGPT** (Hu, Liang, Chen, NDSS 2024).
  - ChatGPT rewrites Ghidra output in three roles, with symbolic
    execution checking that the meaning is unchanged.
  - Its metric counts a name as meaningful if it is within an edit
    distance of a source name, or on a hand-made synonym list.
  - 30.3% meaningful names, against DIRTY's 11.1%, on a small test set.
- **Idioms** (Dramko, Le Goues, Schwartz, arXiv 2025) feeds neighbouring
  functions in the call graph as context. Its variable-name accuracy, over
  variables that can be mapped, is about 13–19% (Table 3a).

### Functions

- **Debin** (He et al., CCS 2018) is the one work that names **globals**
  as well as functions and locals, and the one with ARM.
  - A CRF predicts names jointly over a dependency graph of BAP IR, not
    decompiler output.
  - 3,000 binaries per architecture.
  - Exact-match precision / recall (Table 3): x64 63.5 / 63.1, ARM 61.6 /
    61.3.
  - Its vocabulary is closed: it can only predict names it has seen.
- **NERO** (David, Alon, Yahav, OOPSLA 2020). A GNN over call sites,
  abstracted to the callee's name and its argument values.
  - 67,246 x86-64 functions, split by package.
  - Subtoken F1 45.5 (Table 2), against DIRE 35.5 and Debin 33.7.
- **SymLM** (Jin et al., CCS 2022).
  - Execution-aware embeddings of the function, its callers and its
    callees, through Ghidra.
  - Data: x86, x64, ARM and MIPS, gcc 7.5, O0–O3, 1.43 M functions, split
    by binary, so the same projects are on both sides.
  - Its metric introduced **CodeWordNet**: a predicted word that misses
    still counts if an embedding-plus-edit-distance synonym of it is in
    the truth.
  - F1 0.655 overall; ARM 0.47 (O0) to 0.69 (O2), Table 4.
- **XFL** (Patrick-Evans, Dannehl, Kinder, IEEE S&P 2023). Extreme
  multi-label tagging of name tokens from static features, callers and
  callees, and the whole binary.
  - 741,724 Debian functions.
  - P 0.835 / R 0.575 / F1 0.681 (Table III); F1 0.24 on names never
    seen in training (Table V).
- **SymGen** (Jiang, Jin, Lin, NDSS 2025). Code Llama adapted to
  decompiled code, with callee names normalised.
  - 2.24 M functions over four architectures including ARM, gcc 9.4, with
    duplicates removed across and within splits.
  - CodeWordNet F1 (Table I): x86-64 0.35–0.40, **ARM 0.24–0.28**.
  - SymLM on the same leak-free data falls to about 0.10, from its own
    0.65.
  - It also reproduces AsmDepictor at 0.05 against a self-reported 0.715.
- **ReCopilot** (Chen et al., arXiv 2025).
  - A 7B model trained with continued pre-training, SFT and DPO, given
    data flow and the call graph.
  - ROUGE on 1,038 functions (Table 4): function names 50.6, variables
    43.5. Binary Ninja's Sidekick gets 34.6 and 23.8.
- **NFRE** (ISSTA 2021), **AsmDepictor** (AsiaCCS 2023) and **HexT5** (ASE
  2023) were not read. They appear here only through others'
  reproductions: AsmDepictor via SymGen (above), HexT5 at about 3.0 F1 in
  Shang et al.

### LLMs and benchmarks, 2025–2026

- **REBENCH** (Won, Jin, Ma, Lin, arXiv 2604.27319, April 2026).
  - Built leak-free by construction, over x64, x86, ARM32 and MIPS32,
    O0–O3, with 40 k test samples.
  - Zero-shot open 13–15B models, scored with CodeWordNet F1.
  - Function names reach about 0.05–0.16 on x64 and up to 0.38 on x86,
    lower on ARM and MIPS. Variable names reach about 0.1 at O0 and 0.05
    at O1–O3. Real microcontroller firmware is near zero.
- **Shang et al.** ("An Empirical Study on the Effectiveness of Large
  Language Models for Binary Code Understanding", arXiv 2504.21803, 2025).
  CodeLlama-34B's function-name token F1 is 26.8 on x64 and 27.2 on ARM.
- **DecompileBench** (Gao et al., arXiv 2505.11340, 2025). 23,400
  functions, judged by an LLM on twelve aspects, two of them identifier
  meaning and identifier accuracy.
- **Kwon et al.**, "SoK: AI-Augmented Binary Reversing": a survey; it
  makes the same leakage point.
- **TTPDetect** (Xuan, Xu et al., arXiv 2602.06325, 2026). The one
  explicitly bottom-up LLM pass found.
  - It renames functions leaf-first from summaries of their direct
    callees.
  - Call-graph cycles get placeholder summaries and are revisited.
  - It doesn't score the names themselves.
- **LLM4Decompile** (arXiv 2403.05286) and **Nova** (ICLR 2025) score
  re-executability only, not names.

### What the numbers say for us

- **Expect much less on an unnamed game.** On code a model hasn't seen:
  - variable names score 35–43% exact match;
  - function names score 0.24–0.40 subtoken F1, and ARM is at the low end;
  - zero-shot open models do worse (REBENCH);
  - no study covers Thumb or a GCC 2.x compiler.
- **Context helps.** NERO, XFL, SymLM and GenNm each gain from giving a
  function its callers' and callees' names. That is the bottom-up order the
  naming-sample unit already plans.
- **None of the trained models fits our input.** They read Hex-Rays or
  Ghidra C from x86-64. Ours is Rust from Thumb, which those models have
  never seen. Retraining one would mean building a corpus of agbcc
  binaries with debug info, which only pret's decomps provide, and
  testing it on Emerald would then measure memory. A general LLM prompted
  as an agent is the realistic route; the papers give its method and its
  metric, not a model.

## Scoring names

- **Exact match** is what DIRE, DIRTY, VarBERT, ReSym and Debin report.
  It is simple, but too harsh for us. Emerald's names occur in FireRed's
  `.sym` only 53% of the time, and two humans at pret don't agree on
  `AtkCanceller`/`AtkCanceler`.
- **Subtoken precision and recall** (NERO, SymLM, SymGen, GenNm, REBENCH).
  - Split names on case and `_`, normalise case, then compare multisets
    of subtokens.
  - CodeWordNet adds synonyms (`num`/`count`, `idx`/`index`) through
    embeddings and edit distance.
  - pret's prefix conventions (`g`, `s`, `Task_`, `CB2_`, `sub_`) carry
    information and should be scored as their own token, not stripped.
- **An LLM judge** (GenNm's GPT-4 evaluator, DecompileBench) catches names
  that are good but worded differently. It is not reproducible across
  model versions, so it should be a third number, never the gate.
- **Separate what is easy.** Results should be reported per kind:
  functions, globals, ROM data and locals. Within functions, split leaves,
  wrappers and functions under four instructions, which naming studies
  and FID alike treat as their own case.
- **Leakage**: see below.

## What fits us

### Emerald: names are a lookup

pret's ELF is exact for every function, global and data object, and
DWARF is near-exact for locals. So for Emerald the mod view's source map
is a **translation of pret's symbols, keyed by address and pinned to a
pret commit**, with no recovery at all. The pret matching unit
(`mod/pret-names`) builds that; this review adds:

- Take names from the **ELF, not the `.sym`**: types, the Thumb bit and
  sizes are needed to tell a function from a branch label.
- Take **locals and parameters from DWARF through gdb**. Name a register
  local only over the address ranges where our own liveness agrees with
  DWARF, because agbcc's DWARF gives one location for the whole scope. The
  same holds for stack slots: DWARF names `sp`+offset after the prologue.
- **Qualify the 225 colliding static names**, best with a Rust module per
  source file, using the line table for each function's file.
- **Asm labels** (scripts, maps, songs) go in the same map, for the
  asset and script work later.
- A **pret commit** pins the map. The symbols branch's history gives the
  diff when pret renames: 42 names in a year.

### Other ROMs: match first, then name

For Emerald in other languages, Ruby/Sapphire, FireRed/LeafGreen, and any
agbcc game that links the SDK libraries, matching is the step that
carries names. The literature points to one design, which is a unit of
its own if the owner wants other games named:

- **Hash each function** over our own decoded instructions. Mask `bl`
  offsets (`0x07ff07ff`) and literal-pool words, and keep mnemonics,
  registers and small immediates. This is Ghidra FID's specific hash, done
  the ds-decomp way.
- **Take a name** only on a unique match, or when a FID-style score passes
  about 15 code units. Short functions need a matching parent or child.
- **Pass names on** through `bl` targets and pool pointers to callees and
  data (FLIRT 2.0, decomp-toolkit, ds-decomp). Most of a game's globals
  are reached through a pool word in a named function, so this names data,
  which no learned model does well. Debin is the only one that names
  globals at all.
- **Near matches**: objdiff's penalty diff (it already decodes Thumb with
  `unarm`) ranks candidates for functions that changed. A human or an
  agent confirms them.
- **Score it** on the ROMs where pret has truth: Emerald against FireRed,
  and Emerald against Ruby where Ruby's name isn't `sub_`.

### The naming loop: agents, scored against pret

The naming-sample unit hides pret's names and has an agent name a capped
sample. What the literature says for it:

- **Give it context bottom-up.** Each prompt carries the function's
  generated code, the names its callees and globals already have (pret's,
  or the agent's from earlier rounds), and its callers. Re-run once
  callers are named. Where several functions use one global, vote across
  their guesses (GenNm's consistency vote, ReSym's aggregation).
- **Measure the leak.** pokeemerald has been public for years, and the
  agent will often *recall* `Task_HandleChooseMonInput` rather than infer
  it. Three controls, each cheap:
  - score the same sample with all context hidden (no callee or global
    names), so a high score then is memory;
  - ask the agent whether it recognises the game, and score functions
    whose name it can give with no code at all;
  - include a slice of FireRed or Ruby code, where names partly differ,
    and see whether the agent answers with Emerald's names.
  - The score on Emerald is then reported as an upper bound for an
    unnamed game. REBENCH's leak-free numbers, about 0.1–0.4 F1, are the
    lower reference.
- **Score as above.** Subtoken precision and recall as the main number,
  exact match beside it, an LLM judge third. Report per kind and per size
  class, with tokens and seconds per function.
- **Locals are the hard part.** Learned results are lowest for locals and
  struct fields (ReSym: 16% on fields), and DWARF is truth for only 70%
  of them, the ones in registers. Leave locals to a second round.

### Printing: names change no behaviour

A name, a comment or a layout choice never reaches the emulated machine,
so the readable form can print anything that helps an agent read the
code:

- pret's name, the C prototype and types from DWARF (`struct Pokemon *mon`
  as a comment on `r0`);
- the source `file:line` for each block, from the line table, and the
  local names over their ranges;
- the global's type at each pool load.

Two limits:

- **Legality.** Decision 2 keeps names and addresses in the map. Printing
  pret's own C source lines as comments would put pret's text in our
  output: allowed by pret's licence, but a choice for the design to make
  explicitly.
- **The check.** It must be mechanical. Print with and without the map,
  map each identifier in the named print back to its address key through
  the map's inverse, and drop comments; the two token streams must be
  equal. Frame hashes on the new-game route are the end-to-end check the
  source-map unit already names.

## Sources

Tool sources were read at these commits (shallow clones):

| Repo | Commit |
|---|---|
| NationalSecurityAgency/ghidra | `057a646233` |
| google/bindiff | `13c437afa0` |
| joxeankoret/diaphora | `35c7657081` |
| rizinorg/rizin | `3ac08053a7` |
| encounter/decomp-toolkit | `0645e16511` |
| encounter/objdiff | `9aeec9cbab` |
| ethteck/coddog | `a72e89e435` |
| AetiasHax/ds-decomp | `3b2df91c53` |
| Vector35/warp | `c9cfe5b7a0` |
| pret/pokeemerald (master / `symbols`) | `731ad5bf` / `dba968c6` |
| pret/pokeruby (`symbols`) | `78b19905` |
| pret/pokefirered (`symbols`) | `3b401eaf` |

The papers, with links, are in [references.md](../references.md) under
"Name recovery".
