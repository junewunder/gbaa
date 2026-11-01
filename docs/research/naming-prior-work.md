# Naming stripped code: prior work beyond code-only models

Researched 2026-10-28, after the naming sample (`naming-sample.md`) came
back with Sonnet 5.5 at 0.21 subtoken F1 against pret, and 0 of 30
hand-judged names useful. The question: has anyone named functions and
globals in a stripped binary, a game especially, well enough for a human
reader to use the names, and what did it take? This review covers five
angles: dynamic information and strings, LLM agents with tools, the game
decompilation communities, name transfer between related games and
libraries, and human judgments of usefulness.

The code-only literature (DIRE, DIRTY, VarBERT, ReSym, GenNm, SymLM,
SymGen, REBENCH) and the matching tools (FLIRT, Function ID, BSim,
BinDiff, Diaphora, decomp-toolkit, ds-decomp, coddog) are already
reviewed with numbers in [name-recovery.md](name-recovery.md). This
document cites them only where they bear on the five questions.

Method: web search and fetch; papers read as PDFs where they could be
downloaded (SymGen, SymLM, GenNm, VarBERT, DeGPT, REFORGE, Lacomis's
thesis, Votipka et al.); GitHub PRs and issues read with `gh`. The
Klonoa rename counts in section 3 are my own measurement on a clone of
`Dream-Atelier/kl-eod-decomp` at `45e66dd` (2026-10-05). Where a sentence
is my inference and not what a source says, it says so.

## What this means for gbaa

- **Our 0.21 is the normal result for code-only naming, not a bug in our
  prompt.** REFORGE ran seven LLMs, Claude Sonnet 4.5 and GPT-5.1 among
  them, on stripped functions. Mean token F1 was 0.18–0.29 and the
  median was 0. Once Ghidra's generated names are removed, a function
  body holds a median of two meaningful identifier tokens. SymGen, a
  fine-tuned model, gets 0.26 on ARM. Prompt tuning won't close that gap;
  the evidence has to come from outside the function body.
- **Library code should be matched, never guessed.** Klonoa's decomp had
  Claude name all 475 functions from static code in one day. Since then,
  49 of the 74 surviving names in the m4a sound engine and libgcc have
  been replaced with pret's standard names (66%), against 40 of 374 in
  game code (11%, a floor, since most game names haven't been rechecked).
  Several of the wrong guesses are off by one along a run of functions
  (`m4aSongNumStop` was really `m4aSongNumContinue`). pret's pmd-sky
  named 742 NitroSDK functions by byte-identical code from SDK builds. The
  same work found 44 existing hand-given names wrong, in the same
  off-by-one pattern. For us, m4a, libgcc, libc, `agb_flash` and `librfu`
  come from pret's objects by matching (name-recovery.md, "Function
  matching") and never go to the namer.
- **The most promising idea: name from runtime evidence, with a control
  run.** In Klonoa's PRs #123 and #128, an agent ran the ROM headless,
  intervened on one thing (press START, take a hit, double a scale
  factor), watched which RAM and I/O registers moved, and renamed from
  what it measured. That overturned static names an LLM had given
  confidently: `VBlankCallback_Credits` is the 28 KB player update and
  is never installed as a callback; `HandlePauseMenuInput` is the
  per-frame entity loop. Our position is better than an emulator's. We
  own the recompiled runtime, so we can log every function entry, every
  store and every I/O write per frame without watchpoints, tied to which
  game state is running. My suggestion (an inference, untested): build a
  per-function evidence record (calls per frame, scenes in which it runs,
  I/O registers and RAM regions it writes, ROM tables and strings it
  reads, which input changes its behaviour) and give that to the namer
  beside the code.
- **Strings and the calling context are the cheap static wins.** Adding
  string literals and binary-level context took Hieronym from 0.41 to
  0.53 F1 on x64 (+28% relative). Adding callers and callees added
  another +14%. HyBinMAS reports +18% F1 from execution feedback over
  static analysis alone. Our sample gave neither strings nor callers.
  Emerald's text uses its own character encoding, so the namer needs
  decoded strings (and the asset names from `pokeemerald-assets.toml`)
  shown at each pool load. GenNm and Hieronym both pass names down from
  callers as well as up from callees; a second, top-down pass from the
  task or callback that installs each function is worth adding.
- **The namer should be allowed to abstain.** In the only controlled
  human study of a learned namer (DIRTY, 40 reverse engineers), names
  made no significant difference to correctness or time. On one task,
  plausible names on the wrong arguments led nearly half the participants
  astray. Those who answered wrong trusted the names more. Avidan and
  Feitelson found misleading names worse than meaningless ones, and
  zeldaret's TWW docs say the same as policy. So for us: `unk`/`sub_` when
  there is no evidence, a short ranked list where there is some
  (REFORGE), and the evidence printed next to the name.
- **Dead ends:** training or fine-tuning a naming model (none fits Thumb
  under agbcc, and Emerald is in every model's training data); a bigger
  model on the same code-only input (REFORGE's seven models fall in one
  narrow band); and scoring by token F1 alone. Nobody has measured an
  agent that explores the binary against one-shot prompting on naming, so
  that comparison would be new.
- **pret will not take LLM names.** pokeheartgold bans LLM-generated
  contributions outright. At pokeplatinum, an 80 KLOC LLM rename PR was
  closed after a reviewer called it unreviewable, and the project's
  AGENTS.md requires the human to explain every name. pmd-sky accepted Claude's SDK names only because each was
  byte-verified and checked by two reviewers. Our names stay in our
  sidecar, and pret stays our oracle; we don't send pret contributions.

## 1. Dynamic information and strings

**Execution traces, the academic version.**

- SymLM (CCS 2022) is "execution-aware" only through pretraining. It
  reuses Trex's model, which was trained on micro-traces: short emulated
  runs of code snippets, not runs of the program. In SymLM's own
  ablation, the pretrained weights mostly speed up training (+20–53% F1
  in the first 5–20 epochs); after convergence the gap is small. Calling
  context gave the lasting gain.
  ([paper](https://xinjin95.github.io/assets/pdf/SymLM_ccs2022_paper.pdf),
  RQ4; [repo](https://github.com/OSUSecLab/SymLM)). SymGen later found
  SymLM's 0.73 F1 drops to 0.105 on deduplicated data
  ([SymGen, NDSS 2025](https://www.ndss-symposium.org/wp-content/uploads/2025-797-paper.pdf)).
- HyBinMAS (Wang et al., ASE 2026) is the one naming paper found that
  runs the program. Three agents propose a name and summary, generate
  test executions, and check the behaviour, iterating over an attributed
  call graph. The abstract reports 0.343 average F1 over five
  architectures, and **an 18.14% F1 gain from execution-grounded
  evidence over static-only analysis** on a controlled subset. I could
  read only the abstract (ACM returned 403), so the setup behind that
  number is unknown. ([DOI](https://doi.org/10.1145/3832783.3837532))
- No paper found uses screen contents, game state or memory watching to
  name functions.

**Runtime evidence in a game decomp.** The Klonoa: Empire of Dreams
decomp (GBA, 2001) is the only project found that names from runtime
evidence. Everything here is from its PRs and docs.

- Its `docs/dynamic-analysis/` directory has one script per claim
  ([README](https://github.com/Dream-Atelier/kl-eod-decomp/tree/main/docs/dynamic-analysis)).
  Each script boots the ROM headless in macabeus's TypeScript emulator
  [gba-kit](https://github.com/macabeus/gba-kit), resolves addresses from
  the build's DWARF, and runs an A/B intervention: "change one thing
  between two otherwise identical runs and observe what moves". A header
  comment that cites a script claims exactly what the script prints.
- [PR #123](https://github.com/Dream-Atelier/kl-eod-decomp/pull/123)
  (2026-08-15) renamed four functions this way. For example,
  `UpdateAffineBGParams` became `HBlankBg2RefPointUpdate`: it runs 228
  times per frame with H-Blank armed and 0 times without, and watchpoints
  over all of I/O caught only the BG2 reference-point registers, never
  `BG2PA..PD`. `UpdateHUDTimerAndLives` became `UpdateHUDTimePanel`: the
  function never reads lives.
- [PR #128](https://github.com/Dream-Atelier/kl-eod-decomp/pull/128)
  (2026-08-18) showed that `VBlankCallback_Credits` (0x0800D188) is a
  28,652-byte function with one entry and one exit, called by `bl` every
  frame. No word in ROM or RAM holds its address, so nothing installs it
  as a callback. It is now `UpdatePlayerState`. The same PR showed that a
  field first named `invincibilityTimer` is really `hurtStateTimer`, and
  that 20 of 27 `bl` targets inside it are not functions (agbcc emits
  `bl` for long intra-function jumps). Three adversarial reviewer agents
  were run against the first commit and found its scale wrong.
- The README warns that one earlier round "produced a confidently wrong
  rename because the *script* had two hardware register addresses
  transposed". The observations were right and the labels were not.
- macabeus's write-up of the project
  ([GambiConf, 2026-08-17](https://gambiconf.substack.com/p/starting-a-decompilation-project))
  describes the agent reading a memory address during play and confirming
  with a watchpoint that it changes on damage. It gives no accuracy
  numbers.
- A headless mGBA debugger MCP server with watchpoints exists
  ([mcp-mgba](https://glama.ai/mcp/servers/Canadadry/mcp-mgba)); no
  naming results were found for it.

**Strings and data.**

- Hieronym (arXiv 2609.12457, 2026) gives the model a "global binary
  context": 100 sampled string literals longer than 8 characters, the
  file type and the dynamic dependencies, condensed into an LLM summary.
  On x64, **F1 rises from 0.4117 without it to 0.5256 with it (+27.6%
  relative)**. Callers and callees add +13.8% F1; the gain is smaller at
  O2/O3 because of inlining. There is no strings-only ablation. ARM F1 is
  0.42. ([paper](https://arxiv.org/html/2609.12457v1))
- SemFlow (Cybersecurity, 2026) propagates names over the call graph
  using strings and leftover symbols from the decompiled output, and
  reports +11.6% F1 over SymGen. I read only the abstract.
  ([article](https://link.springer.com/article/10.1186/s42400-026-00602-6))
- NERO's input is call sites abstracted to library-call names and their
  arguments; it reaches 45.5 subtoken F1 (name-recovery.md). Library
  names are the "leaves" of an x86 program. A GBA game has none, but its
  hardware registers and BIOS calls play the same role (my inference).
- Jin et al.'s BinSum benchmark measured how much each kind of symbol
  matters to LLM summaries of decompiled code. Removing all symbols cut
  semantic similarity by 55%; removing function names alone cut it by
  30.3%, variable names 30.0%, types 19.5%. Function names mattered most,
  which supports naming callees first.
  ([arXiv 2312.09601](https://arxiv.org/html/2312.09601v1))
- REFORGE: once Ghidra's generated names are excluded, a stripped
  function body holds a median of **two** meaningful identifier tokens.
  A re-ranker that picks among a model's candidates using in-body
  identifiers does worse than taking the top candidate. The authors
  conclude that closing the gap needs "signals from outside the function
  body". ([arXiv 2607.07738](https://arxiv.org/pdf/2607.07738), §4.4)
- Human reverse engineers start from the same leaves. In Votipka et al.'s
  observation of 16 reverse engineers, 15 begin by listing strings and
  API calls, and while scanning functions they look for APIs (15),
  strings (15), constants (11) and variable names (11). Dynamic methods
  come last, to test hypotheses.
  ([USENIX Security 2020](https://arxiv.org/pdf/1912.00317))

## 2. LLM agents with tools

**The tools exist; measurements don't.**

- MCP servers for every disassembler exist (GhidraMCP, IDA Pro MCP,
  REA), and practitioner write-ups describe agents listing functions,
  following cross-references and renaming one call at a time
  ([DEV, 2026](https://dev.to/kausarraza/ai-reverse-engineering-in-2026-how-llm-agents-drive-ghidra-and-ida-and-where-they-fail-3270)).
  None of them reports name quality.
- Binary Ninja Sidekick names a function's unnamed callees first, and
  its docs say names improve with callee symbols
  ([docs](https://docs.sidekick.binary.ninja/guide/function_callee_naming)).
  Vector35 publishes no accuracy numbers. ReCopilot measured Sidekick at
  34.6 ROUGE on function names against its own 50.6
  (name-recovery.md).
- Archibald and Thijssen drove Ghidra through GhidraMCP with an LLM
  agent. Their first phase, an agent with tools but no metric, gave
  "incomplete coverage and inconsistent improvements". A structural
  metric alone was gamed. Their final readability score combines a
  structural-similarity gate with three readability measures. The
  abstract reports no naming numbers and no human evaluation.
  ([arXiv 2606.06838](https://arxiv.org/abs/2606.06838))
- Reveree (Benkraouda et al., 2026) diagnosed nine frontier models as RE
  agents on 88 CTF challenges. Failures concentrate in the comprehension
  stages, and more budget or reasoning effort rescues few of them. The
  base model matters more than the prompting strategy. Naming isn't
  scored. ([arXiv 2609.01185](https://arxiv.org/abs/2609.01185))
- TTPDetect renames malware functions leaf-first from callee summaries
  as a preprocessing step, but doesn't score the names
  ([arXiv 2602.06325](https://arxiv.org/html/2602.06325v1)).
- FunInferAgent combines caller, callee and in-body context with an LLM
  over IDA, but publishes no evaluation
  ([repo](https://github.com/EleanoreLabs/FunInferAgent)).

**What comes closest to agent versus one-shot.** No study found compares
an agent that explores the binary with one-shot prompting on name
quality. The nearest evidence:

- HyBinMAS: multi-agent with execution, +18% over static (above).
- GenNm propagates predicted names between callers and callees and
  re-predicts. Its ablation credits that propagation, with context-aware
  fine-tuning, for most of its gain over ReSym. Context from five
  call-graph hops works well; ten add little.
  ([NDSS 2025](https://cs.purdue.edu/homes/lintan/publications/gennm-ndss25.pdf), ablation and App. G)
- REFORGE: the right name is often among a model's top three when it
  isn't first. Best-of-three improves token F1 for 13.6–26.7% of
  functions, so a short list for a reviewer beats one answer.
- Klonoa's runtime PRs (section 1) are agent work with tools, judged by
  reviewer agents and a maintainer, and they overturned one-shot static
  names. That is a case report, not a measurement.

## 3. Game decompilation communities

**Klonoa: Empire of Dreams (GBA).** The project's names went through
three stages.

1. **Static LLM names.** On 2026-03-16 Felipe Sanches had Claude Code
   name all 475 functions
   ([PR #45](https://github.com/Dream-Atelier/kl-eod-decomp/pull/45), "100%
   naming coverage"), m4a included
   ([PR #27](https://github.com/Dream-Atelier/kl-eod-decomp/pull/27)). The
   m4a names were inferred from which sound registers each function
   writes.
2. **Transfer from pret.** The next day,
   [PR #53](https://github.com/Dream-Atelier/kl-eod-decomp/pull/53)
   renamed 22 m4a functions to pokeemerald's names. `InitSceneState` was
   `m4aSongNumStart`; `DisableInterruptsForGfxSetup` was
   `m4aSoundVSyncOff`.
3. **Runtime evidence** (PRs #123 and #128, section 1).

My count, comparing the name file at `8f51271` (all functions named) with
`45e66dd` (2026-10-05):

| | named in March | still functions | renamed since | dropped (not functions) |
|---|---|---|---|---|
| m4a + libgcc | 93 | 74 | **49 (66%)** | 19 |
| game code | 382 | 374 | **40 (11%)** | 8 |

- Game-code renames are a floor on the error rate, since most of those
  names haven't been checked against evidence yet.
- Some library guesses slid one function along a run: `m4aSongNumStop`
  is now `m4aSongNumContinue` and `m4aMPlayCommand` is now
  `m4aSongNumStop`.
- The write-up calls Claude's names "a reasonable guess based on the
  assembly patterns".

**pret.**

- [pokeheartgold's CONTRIBUTING.md](https://github.com/pret/pokeheartgold/blob/master/CONTRIBUTING.md#ai-policy)
  (PR #452, 2026-02-15): "We unequivocally prohibit the use of …
  LLMs to generate contributions". It allows two exceptions:
  human-specified tedious tasks and general questions.
- pokeplatinum:
  - [issue #459](https://github.com/pret/pokeplatinum/issues/459)
    (2025): a user proposed ChatGPT de-obfuscation, renaming a function
    `ClearAndUnloadTextures`. The maintainer answered: "This project does
    not and will not ever use direct invocations of LLMs for its work."
  - [PR #1297](https://github.com/pret/pokeplatinum/pull/1297)
    (2026-10-07): an LLM pass over the remaining `unk` symbols, about
    80 KLOC, built matching but unchecked by its author. A reviewer
    replied "Until you have done so, this PR is meaningless to this repo"
    and asked for one source file at a time; the PR was closed.
  - The [AGENTS.md](https://github.com/pret/pokeplatinum/blob/main/AGENTS.md)
    ([PR #1298](https://github.com/pret/pokeplatinum/pull/1298)) requires
    the human to be able to explain "all naming of undocumented functions,
    variables, and structures".
- [pmd-sky PR #301](https://github.com/pret/pmd-sky/pull/301)
  (2026-09-27), "AI-authored — Claude (Opus 5), under human direction":
  - 742 `sub_` placeholders got their NitroSDK names, each verified as
    byte-identical machine code from a compiled SDK build and checked by
    "adversarial review passes through secondary agents".
  - It also corrected 44 names already in the tree. Six families were
    off-by-one slides through a contiguous run, and nine "BSD socket"
    functions were really NitroWiFi `SOC_*`.
  - A maintainer independently confirmed that "No new symbols are
    hallucinated". Merged with two approvals.
- [pokeruby PR #901](https://github.com/pret/pokeruby/pull/901) (open,
  made with Claude) names slot-machine fields by copying pokeemerald's
  `struct SlotMachine`, "which has the same layout at the same offsets".
- [poketcg2 PR #63](https://github.com/pret/poketcg2/pull/63) renames 141
  WRAM labels; the maintainer says verifying it "will probably take me a
  very long time".

**Other projects.**

- [kfiv-decomp PR #81](https://github.com/Voicedrew11/kfiv-decomp/pull/81)
  (King's Field IV, 2026-10-16): DeepSeek workers proposed about 830
  struct-field names from use sites. Each was applied by renaming the field
  to a sentinel and letting compiler errors find every use, so renames are
  type-exact. The build had to stay matching. One wrong pick showed up as
  an asm diff. A separate data-symbol batch kept 32 renames and reverted
  3. No semantic review of the names is reported.
- [gta2_re PR #835](https://github.com/CriminalRETeam/gta2_re/pull/835):
  a rename pass kept all 3,423 matched functions matching and left 148
  fields unnamed where the meaning was unclear. Review caught names that
  contradicted the code, among them inverted tests and a "vertical
  velocity" that wasn't one.
- [zeldaret/tww's decompiling guide](https://github.com/zeldaret/tww/blob/main/docs/decompiling.md)
  says unclear names can wait for a documentation pass, because no names
  are less confusing than wrong names.
- Chris Lewis's Snowboard Kids 2 posts (N64,
  [2025-11](https://blog.chrislewis.au/using-coding-agents-to-decompile-nintendo-64-games/),
  [2026-02](https://blog.chrislewis.au/the-long-tail-of-llm-assisted-decompilation/))
  are about matching. Naming comes up only as a later documentation step,
  run as a Claude skill, because "even renaming a global variable can
  involve multiple steps".
- macabeus's Sonic Advance 3 work
  ([2025-05](https://gambiconf.substack.com/p/development-journey-on-game-decompilation-841))
  kept placeholder names.
- A secondary roundup
  ([capitalandcompute](https://capitalandcompute.net/blog/which-games-has-ai-decompiled/))
  lists Melee, both Snowboard Kids games, SM64 DS and Klonoa as AI-assisted
  matches. It says SM64 DS's matched code is still mostly unconverted to
  readable form, and that Petari allowed AI for naming and cleanup but it
  "never ended up playing a role". I haven't checked these against the
  projects.
- Not found: any automated or LLM naming in the zeldaret OoT/MM, sm64 or
  decomp.me projects; any write-up that measures LLM name quality in a
  game decomp.

## 4. Name transfer across related games and shared libraries

**Between pret's own Pokémon decomps** (measured in name-recovery.md):

- Of Emerald's 15,608 function names, 53% occur in FireRed's symbols and
  28% in Ruby's.
- pokeruby still has 4,097 `sub_` names, though Emerald, which shares
  most of its engine, is fully named.
- The transfer that does happen is by hand, file by file, in PRs like
  pokefirered's "Sync task.c from Emerald" (#425), "Sync link rfu from
  Emerald" (#436) and "Few renamings based on Emerald" (#472)
  ([search](https://github.com/pret/pokefirered/pulls?q=emerald)), and
  pokeruby #901 above.
- My inference: nobody at pret has automated Ruby-from-Emerald matching;
  the leftover `sub_` names suggest it would still pay.

**Shared SDK and library code** is where transfer works best:

- pmd-sky: 742 NitroSDK names by byte-identical code, plus 44
  corrections, accepted by pret (section 3).
- Klonoa: two-thirds of the guessed m4a names were later replaced with
  pokeemerald's (section 3). The PR says MusicPlayer2000 "is shared across
  virtually all GBA games, and pokeemerald has the most complete
  decompilation".
- decomp-toolkit ships 140 SDK signature files for GameCube/Wii, and
  ds-decomp applies a signature only on a unique match (name-recovery.md).
- Hex-Rays reports FLIRT 2.0, which names callees through relocations,
  taking correctly named MSVC ARM functions from 38 to 660 (a vendor
  number; name-recovery.md).

**Between regions or versions of one game:**

- [dw2003recomp issue #56](https://github.com/gascarcella/dw2003recomp/issues/56)
  (2026-10-08) took names from a matching US decomp of the same game by
  address and by struct-field offsets. 232 of 284 fields (82%) got a
  candidate. Candidates were only suggestions ("name only what the code
  confirms"), and review still fixed names that "said the wrong thing".
- [Lombyte PR #165](https://github.com/lombyte-project/Lombyte/pull/165)
  (2026-10-12) applied only verified or inferred names from two reference
  projects (288 globals, 23 shared level functions, 33 fields) and parked
  396 weaker suggestions for review.

**Accuracy numbers.** No GBA-specific precision/recall exists for any
matching tool. Koo et al. (ACSAC 2021) found function-identification
tools rarely benchmarked well (name-recovery.md). The practical numbers
are the counts above.

## 5. Human-judged usefulness

The controlled studies that exist test variable names in x86 C, with
students or professionals answering comprehension questions.

- **DIRTY (variable names and types).** Lacomis's thesis
  ([CMU-S3D-23-103](http://reports-archive.adm.cs.cmu.edu/anon/s3d2023/CMU-S3D-23-103.pdf),
  ch. 5; published as Yang et al., "A Human Study of Automatically
  Generated Decompiler Annotations", DSN 2025,
  [DOI](https://doi.org/10.1109/DSN64029.2025.00026)) ran 40 participants
  (30 students, 9 professionals, 1 other):
  - There was "no statistical evidence that DIRTY has an overall
    meaningful impact on users' correctness or completion time".
  - On one question DIRTY gave reasonable names to the wrong arguments.
    Almost every Hex-Rays-only participant answered correctly, while
    nearly half of the DIRTY group answered wrong (Fisher p = 0.011).
  - Those who got it right read the code; those who got it wrong "took
    the types and names at face-value".
  - Across questions, participants who answered wrongly trusted DIRTY's
    suggestions more (p = 0.025).
  - Users "universally prefer" DIRTY's names over Hex-Rays', but "users'
    perceptions of the usefulness … do not always align with their
    performance".
- **VarBERT.** In a user study with 27 completers
  ([IEEE S&P 2024](https://par.nsf.gov/servlets/purl/10536794)),
  correctness rose from 55.5% to 66.7% and time fell from 124 s to 110 s.
  Intermediate users gained the most time (141 s to 98 s); experts gained
  little time. The 13 functions were chosen to have **at least 75% of
  their variable names predicted correctly**, so this measures good names,
  not the model's average output.
- **DeGPT.** In a task study with 22 participants
  ([NDSS 2024](https://www.ndss-symposium.org/wp-content/uploads/2024-401-paper.pdf), "User Study"),
  accuracy rose from 39% to 88% and time fell from 1,584 s to 840 s. But
  DeGPT also simplifies structure and adds comments, so names alone
  aren't isolated.
- **Avidan and Feitelson** (ICPC 2017,
  [paper](https://www.cse.huji.ac.il/~feit/papers/Names17ICPC.pdf);
  [thesis](https://www.cse.huji.ac.il/labs/parallel/stud/Avidan-MSc.pdf)),
  on source code: 9 professionals; parameter names mattered more than
  locals. Where the original names were poor, there was no difference,
  and the thesis reports misleading names as worse than single letters.
- **Practitioners.** In an HN thread on LLMs over decompiler output
  ([2025-01](https://news.ycombinator.com/item?id=42573232)), mahaloz
  (DAILA's author) found LLM variable names "a bit better than
  auto-generated ones" and comments "hit or miss". Others expected
  naming to stay manual.
- **LLM judges as a stand-in.** Hieronym checked its GPT-4o judge plus
  embedding metric against 1,600 hand-checked predictions: 89.2%
  agreement on name level, 89.8% on token level. REFORGE found 19.3% of
  token-F1-zero predictions semantically close, so token F1 is a lower
  bound. GenNm also uses a GPT-4 judge. None of these asks whether a
  name helps a reader; they ask whether it means the same as the
  original.
- **Not found:** any human study of *function* names (all of the above
  are variables), any study on game code, and any study of names
  produced by an agent.

Taken together (my inference): names help a reader when they are mostly
right (VarBERT's ≥75% sample), make no measurable difference at a model's
real accuracy (DIRTY), and mislead when plausible but wrong. Our 0-of-30
hand judgment fits that pattern. A name shown with the evidence behind it,
or no name at all, is safer than a fluent guess.
