# Finding known functions in other ROMs: a first measurement

2026-10-28, quack. A follow-up to the naming sample (`naming-sample.md`),
which found that a namer lacks the leaves that give code its meaning, and to
the prior work (`naming-prior-work.md`), which says library code should be
matched, not guessed. The question: with pret's names for Emerald, how many
functions can be found byte for byte in other ROMs?

`scripts/libmatch-poc.py` (a proof of concept; its docstring says what it
lacks) turns each Emerald function of 16 or more fixed bytes into a
signature, masking call targets and literal-pool words, and searches each
ROM for it: all 223 library functions (m4a, libgcc, the AGB SDK's flash,
RTC, wireless and multiboot libraries), and every tenth of the 13,313 game
functions.

```
scripts/libmatch-poc.py $GBAA_DATA/names/pokeemerald.toml \
    "$HOME/ROMs/gba/Pokemon - Emerald Version.gba" ROM...
```

| ROM | library (of 223) | game (of 1,332) | library found |
|---|---|---|---|
| Super Mariomon (an Emerald hack) | 216 | 989 (74%) | wireless 110, m4a 57, flash 12, RTC 9 |
| Kirby & the Amazing Mirror | 53 | 5 | m4a 44, flash 2, libgcc 7 |
| Kirby: Nightmare in Dream Land | 48 | 1 | m4a 44, flash 2, libgcc 2 |
| The Minish Cap | 20 | 0 | m4a 20 |
| Metroid: Zero Mission | 10 | 4 | flash 2, libgcc 8 |
| WarioWare, Inc. | 11 | 3 | flash 2, libgcc 9 |
| Emerald Rogue, Emerald Imperium | 15 | 1 | m4a 9, multiboot 6 |

Matches aren't checked for uniqueness, so a few may be false.

- **Same compiler, same engine: most of the game.** Super Mariomon, a hack
  built like pokeemerald, matches 74% of the sampled game functions.
- **Other games: mostly m4a, and only part of it.** Other games ship other
  versions of Nintendo's sound engine. More references would add more:
  decomps of the Minish Cap and Metroid: Zero Mission exist, and
  pokefirered is built here.
- **A modern compiler defeats byte matching.** Rogue and Imperium are
  pokeemerald-expansion built with modern GCC: 1 game function of 1,332.
  Same source, different bytes. Finding those needs matching on structure
  (data flow, constants, strings, call graph), as BSim does.
- **Library code is about 2% of a game, but it's the leaves** (sound,
  saving, division, the link cable), which a bottom-up namer most needs.

Follow-up: `cross-compiler-matching.md` matches on structure instead (IR
fingerprints spread through the call graph): 97.1% precision and 70.5%
recall from agbcc Emerald to the modern-GCC build, and 48% of Rogue named.
