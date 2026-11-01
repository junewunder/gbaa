# Provenance spike: where Emerald's VRAM, palette and OAM bytes come from

Unit `assets/provenance` (decision 2, "Two views of a ROM", june
2026-10-26), with [asset-extraction.md](asset-extraction.md) as its
literature review. The question: if the interpreter tags every byte
written to VRAM, palette RAM, OAM and the sound FIFOs with where it came
from (a ROM address and the transforms on the way), how much of what
Emerald shows traces back to ROM, through what, from how many blobs, and
what stays unexplained. Run 2026-10-27 on quack, Emerald's new-game route
(`units/emerald-newgame.toml`, reset to the truck, 5400 frames).

Rerun it with `just provenance emerald-newgame`
([commands.md](../tools/commands.md)); the reports land in
`out/provenance/emerald-newgame/`.

## How it tags

`runtime::prov` (`crates/runtime/src/prov.rs`), behind the runtime's
`provenance` feature, which only `just provenance` turns on (as
`desktop/provenance`, built into `target/prov`). Every normal build
compiles none of it.

- A shadow `u32` per byte of EWRAM, IWRAM, palette, VRAM and OAM: a
  25-bit ROM offset (or "no ROM") and 7 bits for the steps taken: `cpu`,
  `cpuset`, `fastset`, `dma`, `lz77`, `unpack` (RL, Huffman, BitUnPack,
  diff), `derived` (computed).
- **DMA, `CpuSet`, `CpuFastSet`** copy each unit's tags from the read
  before the write: exact.
- **BIOS LZ77 / RL** (our HLE, decision 12) tag every byte written with
  the compressed blob's first byte: the origin is the blob's address.
- **CPU stores** have no register taint (the interpreter's registers
  carry no tags). Instead the last 8 CPU loads are remembered, and a
  store whose width and value equal one of them takes its tags (a copy:
  `memcpy`, `ldm`/`stm`, struct copies). Any other store is `derived`. As
  a guess at the table it was built from, a derived byte keeps the origin
  of the newest remembered load that traces to ROM and isn't a word that
  looks like a pointer (a literal pool's).
- Counted as they happen: every byte stored to VRAM, palette, OAM and the
  FIFOs by class; and bitmaps of the ROM bytes that reached the first
  three, as copied bytes, compressed blobs' first bytes, and guessed
  tables.

## Results

Non-zero bytes at each checkpoint. "From ROM" is copied or decompressed
from a ROM address, never computed; "computed" is `derived` anywhere on
the way.

| checkpoint (frame) | VRAM non-zero | from ROM | computed | palette from ROM | palette equal to a ROM copy | OAM from ROM |
|---|---|---|---|---|---|---|
| main menu (1000) | 4,036 | 6.4% | 93.6% | 0% | 98.5% | 46.0% |
| Birch's speech (2000) | 23,485 | 20.2% | 79.8% | 9.6% | 90.1% | 46.1% |
| naming screen (3800) | 19,723 | 25.9% | 74.1% | 0% | 99.7% | 44.4% |
| truck (5400) | 21,190 | **90.3%** | 9.7% | 0% | **100%** | 46.0% |

- **VRAM in the overworld is 90% traced.** At the truck: 62.1% LZ77 then
  DMA (`gTilesetTiles_General`, `gTilesetTiles_InsideOfTruck`), 14.5%
  CPU-copied then DMA'd (the metatiles' tilemap entries,
  `gMetatiles_InsideOfTruck`), 11.0% DMA straight from ROM (sprite frames,
  `gObjectEventPic_BrendanNormal`, tileset animations), 2.6% `CpuSet`.
  The 9.7% computed is the text window's tiles.
- **Menus and text screens are mostly computed**: the text. The main
  menu's VRAM is 93.6% computed, 74.6% of it drawn into a RAM buffer and
  moved by `CpuFastSet` and DMA: the font renderer builds each glyph's
  tiles on the CPU. Those bytes are text, not an asset; the font itself
  is a table the renderer reads.
- **The palette is computed everywhere, and equal to ROM data.** Every
  screen fades in, and the fade (`BlendPalette`) writes each colour as
  arithmetic, so no byte of palette RAM is a copy. But at each
  checkpoint 90 to 100% of the computed palette bytes equal, offset for
  offset, ROM-copied bytes in one EWRAM buffer at `0x02037714`:
  `gPlttBufferUnfaded` in `pokeemerald.map`. So "computed" palettes are
  explained by value, against the unfaded buffer the report finds by
  itself (no symbols needed). The report prints this line for palette
  RAM.
- **OAM is half templates, half computed.** 46% is copied from ROM OAM
  templates (`gObjectEventBaseOam_16x32`, `sOam_64x64`) through the
  game's OAM buffer and `CpuSet`; the rest is positions and tile numbers
  the sprite code computes. Not an asset either way.
- **The sound FIFOs are 100% computed**: m4a's mixer output, as expected.
  Music has to be found where the sound driver reads (song headers, the
  voice table), not where it writes.

**Transforms, over the whole route** (every byte stored, not just what
stays): VRAM 4.57 MB stored, 3.7% copied or decompressed from ROM (LZ77
1.8% straight, plus via `CpuSet` or DMA), the rest computed text tiles
and buffer refreshes. Palette 5.5 MB, almost all the per-frame DMA of the
faded buffer.

**Source blobs**, every ROM byte that reached VRAM, palette or OAM by
copy or decompression along the route: **37 compressed blobs** (all LZ77)
and **58 copied ranges** (19,859 bytes; ranges within 16 bytes joined).
Matched against `pokeemerald.elf`'s symbols (`nm`, by hand):

- All 37 compressed blobs start exactly at a pret graphics symbol: title
  screen (`gTitleScreenPokemonLogoGfx`, `sTitleScreenRayquazaGfx`), intro
  (`sIntro1Bg_Gfx`, `gIntroCopyright_Gfx`), Birch's speech
  (`sBirchSpeechShadowGfx`, `gMonFrontPic_Lotad`, `gTrainerFrontPic_May`),
  naming screen, the two tilesets.
- 57 of the 58 copied ranges are in data symbols, most at offset 0 of a
  `_Gfx`, `_Pal` or `_Pic` (`gTextWindowFrame1_Gfx`, `sNewGameBirch_Pal`,
  `gTilesetAnims_General_Water_Frame0`). Partial ranges are sprite
  frames shown (`gObjectEventPic_BrendanNormal+0x300`) and OAM template
  fields. The one in code is 4 bytes of a literal pool.
- The 84 guessed table ranges are half noise: 34 are in code (literal
  pools: a constant then arithmetic), 50 in data (palettes again, the
  sine table, OAM templates, `sCenterToCornerVecTable`). The guess is a
  lead, not provenance.

**Unexplained** (no ROM origin, no value match): text glyph tiles (built
by the font renderer), OAM positions, palette bytes in a fade's middle
(and Birch's speech's 9.6% copied into place mid-fade), the sound mix.
Nothing in VRAM came from a never-tagged source except computed bytes.

## Cost

Emerald's route, 5400 frames, interpreter, `--frame-times` on quack with
the other lane busy: 3.75 to 3.96 ms a frame untagged, 3.91 to 4.26 ms
tagged (two runs each): about 5%, within run-to-run noise. Memory: 1.5 MB
of shadow and 12 MB of bitmaps (statics, touched pages only). Both builds
draw the truck checkpoint's hash (`26c2f9d4009c5696`), so tagging
doesn't change the run.

## What it means for asset extraction

- The BIOS decompressor and DMA hooks alone find every compressed blob
  and nearly every raw graphics blob on the route, at their exact pret
  addresses. This is the cheap, exact core; per-byte CPU tagging adds the
  CPU copies (metatiles, OAM templates) and tells text from assets.
- Palettes need the value match against the unfaded buffer (or a hook
  on the copy into it): the fade hides every palette's origin otherwise.
- Blob boundaries: compressed blobs come with their size (the header);
  copied ranges are only what was shown, so a partly shown sprite sheet
  is a partial range. Joining with the copy's own length (the DMA count,
  `CpuSet`'s r2) would give whole blobs; the spike keeps only bytes.
- The value-match heuristic for CPU stores is the weak point: a store of
  a value that happens to equal a recent load is a false copy (most
  likely for zero, which is why the snapshot counts only non-zero
  bytes). Register taint in the interpreter would be exact, at a cost in
  `armv4t`.
- Not handled: state loads and rewinds (the tags are process-wide), and
  the recompiled build (untested: its loads and stores reach the same
  `Bus` hooks only where it doesn't bypass them).
  Huffman and diff blobs are tagged but untested (Emerald has none).
