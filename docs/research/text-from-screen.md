# A game's text encoding from its screens

Unit mod/text-from-screen, 2026-11-18. The question: for a game with no
community table, can the byte-to-character map be recovered from what the
game shows, by running it? Tried on Metroid: Zero Mission (USA), whose
table (`crates/codegen/text-tables.toml`, the mzm decomp's `charmap.txt`)
was kept hidden till the scoring step. Tool: `crates/text-guess`
(`gbaa-text-guess`). The guessed table is game-derived and lives in the
private mods repo, `mods/metroid/text-guess.tbl`, with the reading behind
each line in its comments.

## Score

`gbaa-text-guess score ROM mods/metroid/text-guess.tbl`, against the
decomp's table, over the 129 strings of 4 or more characters that the real
table finds in the ROM (a run of codes it knows, ending in `FF00`;
Japanese left out, as the readable print leaves it):

| | right | of | share |
|---|---|---|---|
| strings | 120 | 129 | 93.0% |
| characters | 4,616 | 4,645 | 99.4% |
| codes used | 132 | 134 | 98.5% |

Both misses are the same mistake: `0400` and `041F`, which the English
text uses as a space and a question mark (`Save?`, the copy prompt's
padding), are the full-width `　` and `？` in the real table. Their glyphs
look the same as ` ` and `?`; what tells them apart is where they sit (the
first row of the `04xx` block, whose later codes are kanji that only the
Japanese strings use), and that wasn't read off a screen. With those two
written full-width the guess scores 129 of 129. With strings of any length
(`--min 1`) the real table also takes in short runs that aren't text, and
the guess gets 228 of 257 strings and 99.0% of characters; its misses
there add `0042`, the real table's `”` (the guess has `"`), and `005C`, a
hollow square the guess left out.

Control codes are scored as codes: the guess can't know a code's name, so
a code counts as right where the guess also has a code there.

## Method

Each step is a subcommand of `gbaa-text-guess` (`cargo build --release
-p text-guess`; its usage line lists them).

1. **Screens with text.** `gbaa-desktop --headless --dump-at … --dump-dir`
   over the attract loop, then with a short input script (`start@2000`,
   `a@2500` reaches the file screen's Start Game box). Frames with few
   colours are the text screens. The intro types out *EXTERMINATE ALL
   METROID / ORGANISMS ON PLANET ZEBES...* in capitals.
2. **Find the screen's text in the ROM** (`find ROM LINE…`). Plain
   isomorph search (two places hold the same code exactly when they hold
   the same character) found nothing: the intro's font draws small letters
   as capitals, and the string is mixed case. `find --relative` is the
   community's relative search per word: the letters inside a word must be
   as far apart in code as in the alphabet, the word's first letter is left
   free (it may be a capital, in another run), and other characters take a
   code each. `EXTERMINATE ALL METROID` then fits once, at `0x08446180`:
   small letters `00C1`–`00DA`, space `0040`, and the initials give
   capitals at `0081`+. Two of the four lines don't fit, and that is a
   finding too: the screen wraps differently from the string (`organisms`
   ends the string's first line, the screen's second), and the intro's
   font draws the string's comma as a full stop. The screen is evidence for
   letters, not for layout or punctuation.
3. **Where the renderer reads the font** (`font ROM STRINGS --frames A-B
   --press …`). The interpreter runs the scene with every CPU load watched
   (`runtime::debug`); each code it loads from the strings' range is
   paired with the cartridge reads before the next, and each read proposes
   a table base `addr - code * stride` for strides 1 to 512. Over the 16
   codes of the Start Game box, base `0x08415460` at stride 32 explains 14
   (and `0x0840d7b0` at stride 1 is a width table). The intro itself shows
   no string reads at all: its text is drawn ahead of time, out of the
   watch's sight (DMA and the BIOS's decompression calls aren't seen), so
   the menu was the scene to trace.
4. **Draw every glyph and read it** (`glyphs ROM LAYOUT CODES OUT.png`).
   Trying layouts on the sheet settles the glyph: 8x16, 4bpp, the lower
   tile `0x400` bytes on, so only codes whose bit 5 is clear are glyphs.
   The sheets, 16 codes a row, read as: `0040`–`005F` ASCII's `0x20`–`0x3F`
   (so `004C` is the comma the intro showed as a full stop), capitals,
   then `[ ¥ ]` (JIS X 0201's order, `¥` for `\`), small letters, `{ | }`,
   half-width katakana in JIS X 0201 order (`0141`+, `0180`+), voiced kana
   (`01C0`+), symbols and button pictures (`02C3`+, `0300`+), and a block
   at `0400` that repeats the punctuation row. The agent reading the sheets
   was this session; the reading is the table's comments.
5. **Codes the text uses that no glyph explains** (`unknown ROM TBL`):
   strings found with the guess, letting unknown codes in up to a third of
   a string, and each unknown code with the first string it's in. That
   sorted out the rest: `FE00` new line and `FF00` end (where the screen's
   lines break and its text ends), the area names' katakana (Brinstar's
   second spelling `01CC 0198 019D 015D 0180` reads ブリンスタ through the
   sheet, a check on the kana reading), button pictures in prose
   (*Double-tap `0302` to take ball form*: `0302` is the d-pad's down),
   and codes whose low byte is a value, seen with many values each: `80nn`
   (a column, after a new line), `81nn` (a colour: `8106` before *red
   hatches*, `8100` after), `83nn` (where a Yes or No goes).
6. **Score** (`score ROM TBL`), the only step that reads the real table.

## What it took, and what stops it elsewhere

- The step that does the work is the font: one traced menu box gives the
  font's address, and from there every code's glyph can be read, used on a
  screen or not. Matching screen text to ROM bytes only seeds it (it gives
  a few codes to fit the font with, and the end and new-line codes); the
  `font` step could be seeded from the trace alone, since a text box's
  codes are the halfwords the renderer loads just before its glyph reads.
- A screen can mislead: a font that draws both cases alike, a comma drawn
  like a full stop, text re-wrapped. Read the glyphs, not the screen, for
  everything but finding the strings.
- Two glyphs that look alike (a space and the full-width space, `?` and
  `？`) are told apart only by where they sit in the font; a reader has to
  weigh that, and here didn't.
- A game that compresses its font, or copies it to VRAM by DMA before
  drawing, shows no glyph reads to the watch; then the font has to be
  found in VRAM (tile data that changes as text appears) instead.
- A game with a variable-width or 1bpp font needs `glyphs` to learn those
  formats (it draws 4bpp tiles only).
- Japanese text wasn't read: hiragana (`02xx`) and kanji (`045x`+) are
  hundreds of glyphs, and the readable print leaves Japanese out anyway.
