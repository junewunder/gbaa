# Asset extraction: graphics and music out of a ROM

The design behind decision 2's asset half ("Two views of a ROM", june
2026-10-26), group `assets`, unit `assets/design`. It rests on the
literature review ([research/asset-extraction.md](research/asset-extraction.md))
and the provenance spike ([research/assets-provenance.md](research/assets-provenance.md)),
and it is Emerald first, with pret's pokeemerald (`$GBAA_DATA/src/pokeemerald`,
`731ad5bf`, built) as the oracle. Written 2026-10-27 on quack.

The rule from decision 2: no hand reading of a game's code. Assets are
found by watching where the game's data goes at run time: what the BIOS
decompresses, what DMA and `CpuSet` copy, what the PPU then draws with
which palette, and which songs m4a plays. Nothing here changes a play-view
build: recording is behind the runtime's `provenance` feature, and overrides
are off unless a mod folder is given.

## Summary

- **Two passes: record, then export.** A recording run (the interpreter,
  `provenance` build, a route's presses) writes a **load log** (every BIOS
  decompression, DMA, `CpuSet` and `CpuFastSet` whose source is ROM, with
  its full length) and a **use log** (per frame, which loaded blob each
  shown BG and sprite tile came from, with which palette blob). An offline
  exporter, a new `crates/assets` (`gbaa-assets`), reads the ROM and the
  logs and writes files and a manifest. No emulation in the exporter, so
  export and the pret checks rerun in seconds.
- **The asset unit is the source blob**, keyed by its ROM address: a
  compressed blob (LZ77 or RL, the header gives its size; the decompressor
  says how many bytes it read) or a copied range (the length from the DMA
  count or `CpuSet`'s r2, not only the bytes shown, the spike's note). All
  37 of Emerald's compressed blobs on the new-game route start exactly at a
  pret symbol, so a blob is one of pret's files.
- **Palettes are paired by use.** A tile blob's palette is the palette blob
  whose colours are in the bank the PPU draws it with, found through the
  game's unfaded buffer by value (the spike's match, 90 to 100% at every
  checkpoint) and then that buffer's own tags back to ROM.
- **Formats:** 4-bit (or 8-bit) indexed PNG for tile sheets, JASC `.pal`
  for palettes (pret's format), raw `.bin` for tilemaps, Aseprite for
  sprites with their animation frames and timings, format-1 MIDI at 24
  ticks a beat for songs, WAV for samples, an SF2 soundfont from the
  voicegroups so the MIDI plays with Emerald's instruments, and a TOML
  manifest by ROM address that the source map names.
- **Checked against pret three ways:** the bytes (our decoded tile data
  equals pret's PNG through gbagfx with its `INCGFX` flags), the palette
  (our pairing equals pret's), the image (same size and pixels). Plus
  **rebuilds**: our file through pret's own tool gives the ROM's bytes.
  Songs: our MIDI through `mid2agb` with `midi.cfg`'s flags equals pret's
  built `.s`. Samples: our WAV through `wav2agb` equals pret's `.bin`.
- **Overrides are served at the load, keyed by ROM address.** When the
  BIOS decompresses a blob that has an override, it writes the override's
  bytes instead; a raw blob's override replaces its bytes in a patched copy
  of the ROM image. No ROM is written or grown, so a compressed asset has
  no size limit beyond its unpacked size, which the game's buffer fixes.
  The lit review's content-hash keys suit emulators that only see VRAM;
  ours owns the BIOS and DMA, so it sees the source address, which is
  exact and costs one lookup a load.
- **Coverage is what a route shows.** The new-game route shows about 100
  of pret's ~7,600 graphics; every song the game plays is found and the
  whole song table from it. How to widen graphics coverage (more short
  scripts from crafted saves, a static scan for compressed blobs) is
  june's call, asked as "Graphics coverage" below.
- **The group becomes eleven units, eight of them open** (table at the
  end): the load log is new and comes first, graphics splits into tile
  sheets and sprites, music into songs and samples.

## Recording

### The load log

`runtime::prov` already hooks every BIOS call (`enter_bios`), every DMA
(`enter_copy`) and the bus's reads and writes. The load log adds one record
a call, written as the call ends:

| field | from |
|---|---|
| `frame` | the frame counter |
| `op` | `lz77`, `rl`, `huff`, `bitunpack`, `diff`, `dma`, `cpuset`, `fastset` |
| `src` | ROM address (or a RAM address with its tag's origin, for the second step of decompress-then-copy) |
| `src_len` | compressed: bytes the decoder read, header included, rounded to 4 as pret pads; copies: count × unit |
| `dst`, `dst_len` | destination and bytes written |
| `origin` | for a RAM source, the ROM origin its tags give, and their `via` bits |

Only records whose source traces to ROM are kept. A RAM-to-VRAM copy of a
buffer the BIOS filled from ROM is the common two-step case (62% of the
truck's VRAM): the record names the RAM range and its ROM origin, so the
exporter joins it to the decompression that filled it.

The HLE decoders (`crates/runtime/src/bios/decomp.rs`) already know where
they stopped reading; they return it to the hook. DMA knows its count and
unit; `CpuSet`'s length is r2's low 21 bits and its unit bit 26,
`CpuFastSet`'s is words in 8s.

A copy's `src_len` is the whole transfer, so a sprite sheet DMA'd a frame
at a time (`gObjectEventPic_BrendanNormal+0x300`) shows as several
records inside one pret symbol. The exporter keeps each record's range
and merges ranges that touch or overlap into one blob only when they are
the same kind of asset (below).

### The use log

At each VBlank the recorder reads the PPU's state, keeps only what changed
since the frame before, and appends:

- **BG uses:** for each BG enabled in `DISPCNT`'s mode, its `BGxCNT`
  (character base, 4 or 8 bpp, screen base, size). For each map entry
  shown, the tile's VRAM address, its palette bank, flips, and the tags of
  the tile's first byte and of the map entry (so the map itself traces to a
  tilemap blob).
- **OBJ uses:** for each OAM entry not disabled: index, shape and size,
  1D or 2D mapping (`DISPCNT` bit 6), tile number, 4 or 8 bpp, palette
  bank, flips, affine, position. With the tags of each of its tiles' first
  bytes.
- **Palette banks:** for each of the 32 banks, the origin of its 16
  colours: its tags if copied, else the RAM buffer whose bytes equal it
  offset for offset (the spike's value match, which finds
  `gPlttBufferUnfaded` at `0x02037714` by itself) and that buffer's tags.
  A bank mid-fade matches nothing and is skipped that frame.

A tile's tag gives its blob and its offset in the blob. So a use record
says: tile `k` of blob `B`, drawn with palette blob `P` (bank offset `o`),
as part of BG `n` or OAM entry `i`. Deduplicated by (blob, tile, palette,
use) with a count and first frame, the use log is a few thousand lines
for a route, not a frame-by-frame dump.

### Where it runs

The interpreter build with `provenance` on, as `just provenance` builds it
(`target/prov`). The recompiled build isn't recorded (the spike didn't test
it, and recording needs only to see the data once). New recipe:
(Since tested: provenance works in the recompiled build too, at a cost a
player doesn't notice; "Extract while playing" below.)

- `just assets-record ROUTE`: runs the route, writes
  `out/assets/<game>/<route>/loads.tsv` and `uses.tsv` (Rust, in
  `desktop::prov_report` beside the spike's report).

Logs from several routes of one ROM merge: the exporter takes every
`out/assets/<game>/*/` it finds for that ROM's hash.

**As built (assets/load-log, 2026-10-28):** `gbaa-desktop --assets-log
DIR` (`crates/desktop/src/assets_log.rs`), with `runtime::prov` logging
each load as its call ends (`bios_loaded`, `dma_loaded`; lengths from the
call's registers, or for BitUnPack and the diff unfilters the span its
reads covered). Decompressed bytes keep their offset in the unpacked blob
in a second shadow (`prov::offset_at`), so a tile's tag gives blob and
offset even after a RAM-to-VRAM copy. A RAM source's record lists its ROM
runs (up to 1,024), not one origin: the metatile tilemaps are a few
hundred runs a copy. A load repeated unchanged (the OAM buffer, the
tilemaps every frame) is written once. The use log is aggregated, one
line per distinct (layer, bpp, OBJ size and part, tile source, bank,
palette source) with first and last frame and frames shown; the sprites
unit will want the per-frame OAM sequence for timing, which this doesn't
keep. The palette buffer is found by value as the spike did, then reused
while it matches. Check: `gbaa-assets check-load-log`
(`crates/assets/src/loadlog.rs`); results in docs/tools/commands.md.

### Music: the m4a recorder

Music needs no tags (the FIFOs carry the mixer's output, the spike's 100%
computed). It needs m4a's state, read without side effects once a frame:

- The word at `0x03007FF0` is `&gSoundInfo` once `SoundInit` has run (a
  BIOS convention every m4a game follows, not an Emerald address). Its
  `ident` is `0x68736D53` or one more.
- From `SoundInfo.musicPlayerHead` (+0x24), each `MusicPlayerInfo` by
  `musicPlayerNext` (+0x3C); its `songHeader` (+0x00). A header not seen
  before is logged with the frame: `out/assets/<game>/<route>/songs.tsv`.
  Cries are RAM headers (`gPokemonCrySong`); their voice pointer is still
  ROM, so the sample is found, but the song isn't exported.
- This runs in any build (it only reads guest RAM), as a recorder flag of
  `gbaa-interp` and the desktop's headless mode: `--m4a-log FILE`.

**The song table** is found from the first header logged: scan the ROM for
an aligned word equal to that address followed by `u16 ms == me` and
padding zero; walk back and forward while entries are valid (agbplay's
`IsValidSongTableEntry`: a ROM header pointer whose voicegroup and track
pointers are ROM addresses). On Emerald that gives `0x086B49F0` and its
610 entries. The table, not the route, decides which songs are exported:
all 530 real ones, whether the route played them or not.

### Extract while playing

june 2026-10-29 (`assets/play-extract`): a flag so that as a player plays,
every asset they see is extracted, adding to what's already exported.
The general answer for a game with no decomp and no route: playing is
the route.

**Provenance in the recompiled build works.** Recompiled code does its
loads and stores through `Bus::load` and `Bus::store`, the same calls the
interpreter makes, with the same `prov` hooks; BIOS calls and DMA are
runtime code in both builds. The one gap is codegen's const-prop: a load
it folded (`Code::folded`, literal pools and tables) never reaches the
bus, so a store of its value finds no load to copy from and counts as
computed. Measured on the new-game route (5,400 frames, `--assets-log`,
the interpreter against `out/emerald-prov` built with
`--features desktop/provenance`): `uses.tsv`, `oam.tsv`, `reached.tsv`
and `shown.tsv` identical, every checkpoint frame identical, `gbaa-assets
check-load-log` passes on both, and `loads.tsv` differs in one run of one
DMA (a halfword the recompiled build saw copied by a word load: of
21,190 non-zero VRAM bytes, one more counts as from ROM). So the export
from either build is the same.

**Its cost** (quack, 2026-10-29, another lane busy, so the interpreter's
numbers are noisy):

| | plain | provenance |
|---|---|---|
| recompiled, mean frame (route, recording) | 0.75 ms | 1.05 ms (+40%) |
| interpreter, mean frame | 3.7 to 6.1 ms | 4.7 to 5.5 ms (within noise to +25%) |
| recompiled build, from clean | 293 s | 355 s, its own `target/recompiled-prov` |
| recompiled binary | 130 MB | 139 MB |
| resident memory (interpreter, recompiled) | 24, 45 MB | 28, 53 MB |

A frame is 16.7 ms, so either build records at full speed with room to
spare. The tags are static arrays (two `u32` shadows of RAM, palette,
VRAM and OAM, and three bitmaps of the ROM), touched only where written.
Off without the feature, as before: a play-view build pays nothing.

**As built:** `gbaa-desktop --extract DIR` in a provenance build (`just
play-extract [ROM] [OUT] [RUNNER]`, interp or recompiled), windowed or
headless:

- Each session records into `DIR/play-YYYYMMDD-HHMMSS/` with the same
  recorder as `--assets-log` (`loads.tsv`, `uses.tsv`, `oam.tsv`,
  `reached.tsv`, `shown.tsv`), so a session is one more route folder.
- **F6** writes the logs as they stand and exports; so does closing. The
  export is the CLI's own (`assets::export::graphics`, moved out of
  `gbaa-assets graphics`): every session and route under `DIR`, so
  sessions add to one another and to `just assets-graphics`' output.
  0.2 s for Emerald's two routes. It refuses a `DIR` whose manifest is
  another ROM's.
- **Names** come from `DIR/manifest.toml` (`export::manifest_names`), so
  files an earlier export named after pret keep those names; a blob new
  to this session gets a kind and address (`gfx_08d98e84`), which `just
  assets-graphics` with a source map renames. A game with no decomp has
  only these. Sprite sheets are bounded by the manifest's blob sizes in
  place of the source map's symbols.
- **State loads and rewinds** forget every tag (`prov::forget`): the
  memory they described is gone, so a tile shown after a load counts as
  nothing until the game loads it again, never as what was there before.
- Not with `--assets-log`, `--assets-mod` (it would export the edited
  bytes), `--link` or `--server` (the tags are one machine's).
- **Music** isn't in the flag: hearing one song finds the song table,
  and the table gives every song (`just assets-songs`, a 300-frame run,
  then `assets-samples`), so a session would add nothing.
- Bytes the game's own code copies from ROM (a font's glyphs) are no
  load, so they aren't exported, here or from a route.

Check: `just play-extract-check`, a copy of `out/assets/emerald` (the
new-game route's named export) and a headless session into it: the
route's presses, then Start and the Bag in the truck, 6,800 frames.
`gbaa-assets check-extract` (`export::check`): every tile and palette
blob the session showed and loaded has a manifest entry with its files,
every earlier entry is kept under its name, and at least 5 are new.
2026-10-29: 95 blobs shown, none missed (12 CPU copies listed), 106
earlier entries kept, 10 new: the Bag's sprite sheet (`gBagMaleTiles`),
its screen's tiles and tilemap (`gBagScreen_Gfx`,
`gBagScreen_GfxTileMap`), the list's scroll arrow and four palettes.

## Graphics

### Tile sheets

Every tile blob becomes one sheet:

- **Tiles in blob order**, 4 bpp unless a use drew it as 8 bpp. Row
  width: 16 tiles (128 px, pret's tileset width) for BG blobs; for sprite
  blobs, the frame layout below.
- **Palette:** the palette blob most uses paired with it, written into
  the PNG's PLTE (16 or 256 entries, `x*255/31`, gbagfx's conversion,
  index 0 transparent via `tRNS`). Every other pairing seen is listed in
  the manifest, so a shiny palette or a time-of-day palette is one click
  in an editor, not a second image (the lit review's lesson: images stay
  indexed). A blob never drawn while its bank was readable is written
  greyscale, as gbagfx does without `-palette`.
- **Files:** `tiles/<name>.png`, `<name>` from the source map's `[data]`
  entry at the blob's address (`gTitleScreenPokemonLogoGfx`), or
  `gfx_<address>` without one.

**Palettes** are their own assets: `palettes/<name>.pal` (JASC, as pret
commits them) for each palette blob, raw or LZ77.

**BG tilemaps:** a blob whose bytes reached a BG's screen base becomes
`tilemaps/<name>.bin` (raw 16-bit entries for text BGs, 8-bit for affine,
as pret's `.bin`), plus `tilemaps/<name>.png`, the map drawn with its
tiles and palettes, to look at. The overworld's map is built by the CPU
from metatiles (the truck's 14.5% CPU-copied); metatiles are tilemap
pieces, exported as `.bin` like pret's `metatiles.bin`, but the map
layouts behind them (`data/layouts/`) are game structures, out of scope.

**As built (assets/graphics-export, 2026-10-28):** `gbaa-assets
graphics` (`crates/assets/src/graphics.rs`), `just assets-graphics`. A
blob is tiles when a use shown more than one frame drew from it (a
one-frame use is a scene mid-load: the copyright tilemap's bytes read as
tiles on frame 14), a palette when a shown bank's colours came from it,
and a tilemap when it reached BG VRAM as neither, unless it was written
within 60 frames over VRAM a tile blob had just filled (the tileset
animation frames, never drawn on the route, are tiles). Raw copies merge
only where they overlap: copies that just touch were separate pret
symbols (the naming screen's sprites, the water frames). Sheets are 16
tiles wide unless 8 to 16 divides the count. A tilemap is drawn with the
BG tile blob whose uses show most of its (tile, bank) pairs; 8bpp and
affine maps aren't drawn right yet. The names layer is
`$GBAA_DATA/names/pokeemerald-assets.toml`, only the blobs pret's map
doesn't name. The songs and graphics exports each replace only their own
sections of `manifest.toml`.

**Against pret (the new-game route):** 59 tile sheets, 22 palettes and 12
tilemaps; 85 joined to a pret `INCGFX` file (of its 6,280). Bytes 79,
rebuilds 80 (all 12 tilemaps and every compressed sheet both), palette 25
of the 31 drawn sheets (colour 0 of a 4bpp sheet not compared: pret's
PNGs hold any colour there), image 11 of 57. Misses: bytes, where the game
copies part of pret's file (the naming cursors, 4 of 5 tiles) or one
sprite frame of a strip (Brendan's walking frames: the sprites unit);
palette, sheets drawn with several banks whose PNG carries another (the
tilesets, the intro's background) and the two 8bpp title sheets; image,
pret's layout width (`-mwidth`, 64-px pics, 1-tile-wide strips), which
the route doesn't show. Not joined: 8, palettes inside pret's palette
arrays (the tilesets', the title's) and two sprite frames inside a strip.
`gbaa-assets check-pret graphics`, `just assets-check-graphics`, writes
`out/assets/emerald/pret-check-graphics.txt`.

### Sprites and animations

A sprite is what OAM shows, assembled from its tiles:

- **One OAM entry** of shape × size `w×h` tiles, 1D mapping: tiles
  `t … t+w·h−1` row by row; 2D: rows 32 tiles apart. The entry's tiles'
  tags give blob `B` and offset `f`: the frame starts at `f` in `B`.
- **Frames of one sheet:** every offset `f` seen for blob `B` with the same
  `w×h` is a frame. The sheet's PNG is the frames side by side in blob
  order, `w·8` px apart: that is pret's `-mwidth w -mheight h` layout
  (Brendan's `walking.png`, nine 16×32 frames, 144×32).
- **Both ways games animate** give the same answer through the tags: a
  whole sheet decompressed once and OAM's tile number stepping through it,
  or one frame's VRAM slot refilled by DMA from a different ROM offset each
  time (Emerald's object events). Either way the tile's origin offset
  changes.
- **Timing:** following one OAM index whose blob stays the same, the
  sequence of offsets and how many frames each stayed is an animation.
  Aseprite frames get durations in ms (frames × 1000/59.7275, rounded),
  with the exact frame count in user data. Each distinct sequence (walk
  down, walk left) is an Aseprite tag over its frames; sequences seen
  only once or cut off by the route's end are kept but marked partial.
- **Several entries moving together** (a 64×64 sprite made of four 32×32,
  the title screen's Rayquaza): entries whose tiles come from one blob in
  the same frame, and whose relative positions stay fixed, are composed
  on one canvas. A first cut may export them per entry; the report counts
  composed sprites apart.
- **Files:** `sprites/<name>.aseprite` (indexed, the paired palette, one
  layer, a cel a frame, tags per animation, user data on the sprite:
  ROM address, palette address, bank, `w×h`, mapping), and the sheet as
  `tiles/<name>.png` like any blob.

The Aseprite writer is `aseprite-io` 0.2.0 (MIT/Apache, writes indexed
files with frames, durations, tags and user data). It is young; the first
unit round-trips every file it writes through its reader and through
`asefile`, and if that fails writes the format itself (header, frame,
palette `0x2019`, layer, cel type 2, tags, user data: a few hundred
lines).

**As built (assets/sprites, 2026-10-28):** `crates/assets/src/sprites.rs`,
run by `gbaa-assets graphics` (`just assets-graphics`) from each route's
`oam.tsv`: the recorder (`crates/desktop/src/assets_log.rs`) writes a line
each time an OAM entry changes (place, size, tile, bank, flips, its first
tile's and bank's ROM sources) and `hidden` when it stops showing. Raw
frames one entry shows in a row, the same size and a whole number of
frames apart, are one sheet, widened to the source map symbol holding
them (never joined across two symbols: the naming screen's arrow and
underscore). A sheet is laid a frame across only when its slot was
refilled from ROM (the tile number stayed, the frame changed): pret's
object-event strips. Refilled from a decompressed buffer (a Pokémon's
pic) or stepped through in VRAM, it's laid down, as gbagfx reads a PNG
one frame wide. A blob shown at two sizes, also a BG's, in 2D mapping, not
whole frames, or shown from mid-frame isn't a sheet (`sprites.txt` says
which). Animations: an entry's runs of one frame on one sheet; a loop is
the shortest period seen twice in a row that four fifths of the run
follows, a step one or two along it (a frame dropped on a lag frame),
each step held its most common hold; a sequence played once is kept
unless a loop or a longer one holds it. Partial: a loop not seen whole
twice, or a sequence cut by the route's end. Composed: two entries on one
sheet the same offset apart for 30 frames or more together (see below). The
Aseprite file: the sheet's frames (100 ms, tag `sheet`), then each
animation's steps as linked cels (flipped steps drawn flipped), durations
from the video frames held, which cel user data gives exactly. aseprite-io
0.2.0 writes it and passes the round trip through its reader and asefile
0.3.8 (size, frames, durations, tags, user data, every frame's pixels).

**Against pret (the new-game route):** 15 sheets, 7 animations, 5 OAM
blobs not a sheet (the naming screen's buttons, made of entries of two
sizes; the title's PRESS START, shown from mid-frame). Object-event and
Pokémon strips at the image level 3 of 3, the sheet and every frame:
Brendan walking (9 frames across; walk in place on the naming screen, the
loop 0, 3, 0, 4 at 8 frames each, pret's `sAnim_GoSouth`), the moving
box, Lotad (2 frames down). Other sheets 7 of 12: misses are the
composed intro logo and title, and the naming cursors pret's PNG holds a
fifth tile of. The graphics check gains from the joined and laid out
sheets: bytes 81, rebuilds 81, image 20 of 57. `gbaa-assets check-pret
sprites`, `just assets-check-sprites`, writes
`out/assets/emerald/pret-check-sprites.txt`.

**Composed (assets/sprites-composed, 2026-10-28):** a composed sheet's
Aseprite canvas holds the whole sprite. Its arrangements: at each moment
every entry showing one of its frames, placed from the top-left one (OAM
slots are reassigned under a sprite, so entries aren't followed by
index); each distinct one is a frame after the sheet's, tag `composed`,
each entry a cel on its own layer (`part 1`, …) with `frame=K [hflip]
at=X,Y video_frames=N` in cel user data; the sheet's frames stay on layer
`sprite` at the canvas's top left. Its per-entry animations are dropped
(its arrangements are its animation). An arrangement showing each of the
sheet's frames once, in order, a frame across or down, lays the sheet's
PNG out as that sprite (pret's `-mwidth 8 -mheight 4`
`emerald_version.png`); else it stays plain. `[sprites]` gives `canvas`
and `arrangements`, `layout = "plain"` for the latter. An override reads
the sheet's frames from the top left, and a part that differs from the
ROM's frame as an edit of that frame (unflipped). On the new-game route:
the title's EMERALD VERSION (one arrangement, 128x32, now `image` ok
against pret) and the intro's GAME FREAK letters (six arrangements as
they appear, 160x16). pret's `drops_logo.png` is the plain sheet 4 tiles
wide; ours is 16 wide, so it still misses there.

## Music

### Songs to MIDI

Each song table entry's header gives the voicegroup and each track's
bytecode. The decoder follows `MPlayMain`'s rules (research doc,
"Bytecode"): waits, running status from 0xBD up, notes with gate time,
TIE and EOT, PATT and PEND (one level, as m4a), REPT, GOTO.

- **Format 1, 24 ticks a beat**, one MIDI track per m4a track, as pret's
  `.mid` files are and as mid2agb reads them. Written by a small SMF
  writer of our own (`crates/assets/src/midi.rs`; `midly` isn't vendored
  and the writer is 60 lines). Track 0 is a conductor (tempo, time
  signatures, loop markers; it has no notes, so mid2agb makes no track of
  it), then m4a track *n* on MIDI track *n*, channel *n* − 1.
- **Commands map to what mid2agb reads back** (its `agb.cpp:349–414`):
  VOICE as a program change, MOD CC 1, VOL CC 7, PAN CC 10 (`c_v` + 64),
  BEND as pitch bend, BENDR CC 20, LFOS CC 21, MODT CC 22, TUNE CC 24,
  LFODL CC 26, MEMACC CC 12 (params in CCs 13–15), XCMD CC 29 (command in
  CC 30), PRIO CC 33, a label CC 17. TEMPO as a tempo meta event (BPM =
  arg × 2).
  - mid2agb prints VOL as `value*<song>_mvl/mxv`, the song's master
    volume from `midi.cfg`'s `-V`. The ROM holds the product, so the
    exporter writes the stored byte (what the GBA plays). **Finding (the
    songs unit):** the check passes `midi.cfg`'s own `-V` and first sets
    each CC 7 to the least value that scales back to the stored byte
    (always possible, since `127/mvl ≥ 1`). `-V127` can't work: where a
    track has no VOL before its first note, mid2agb writes `VOL
    127*mvl/mxv` itself, before the KEYSH (59 of pret's 2,082 tracks), and
    with `-V127` that would be 127, not the ROM's mvl. The exporter marks
    such a VOL (first command, KEYSH next) with a text event `mid2agb VOL`
    and keeps it as CC 7 so a player hears it; the check drops it.
  - mid2agb writes `KEYSH , <song>_key+0` at each track's start itself
    (529 of pret's 530 `.s`), so a KEYSH there with argument 0 is
    dropped. Any other KEYSH, and REPT and nested PATT, have no
    controller in mid2agb: they are written as MIDI text events (`KEYSH
    2`), and songs using them are reported apart, as not expected to
    rebuild.
- **Bar lines (finding):** mid2agb splits waits at each bar line and
  makes its patterns (PATT) a bar each, so the bytes depend on the MIDI's
  time signatures, which the ROM doesn't hold. The exporter infers them
  (`songs::bars`) as a list of restarts, each (clock, bar length): a guess
  is tested by writing the waits as mid2agb would (a split at each line,
  then `SplitTime`'s) against the ROM's, and by every pattern, each PATT
  and the first time through its target, running from a line to the next
  (or to a loop marker, an EOT, which ends a pattern too, or the end). Left to right from the first clock
  the bars so far can't explain, a restart goes at a place every track has
  a boundary at; of the lengths a time signature gives (n/4 to n/32), the
  one that explains the rest of the song is taken (4/4, else the shortest),
  else one a 4/4 restart after explains the rest with (the pickup bar
  pret's MIDI often has), else the one that explains furthest. They go in
  the conductor as time signatures, and in the manifest as `bars = [[clock,
  length], ...]`. With 4/4 everywhere, 217 songs were exact; with one
  phase, 492; with restarts, 526 of 530; with the markers below, 530.
- **Markers (finding, 2026-10-28):** pret's MIDIs hold things mid2agb
  writes as nothing, and they still shape the bytes. A stray controller
  (`mus_route104`'s CC 13) splits a wait with no command there; a
  difference that doesn't reach the ROM (`mus_sailing`'s VOL 58 and 59,
  which scale to one byte; `mus_encounter_champion`'s bend LSB) stops
  mid2agb making a PATT of a bar equal to an earlier one. The exporter
  writes CC 3 (undefined in General MIDI; mid2agb writes it as a split
  and nothing else) for both. Value 0 marks a split the bars can't
  explain (`songs::marks`; the bars are first sought without these, and
  allowed them only when no bars explain the song otherwise:
  `mus_mt_chimney` needed them to find pret's restart at 2352). Other
  values keep a bar apart: the exporter runs mid2agb's own steps up to
  `Compress` on its MIDI (`crates/assets/src/mid2agb.rs`), and where it
  would make a PATT the ROM doesn't have, gives the bar, and each bar the
  ROM made a PATT of it, a value of their own, and runs it again. Only
  the 4 songs above get markers.
- **The loop:** GOTO's target and the GOTO itself as the `[` and `]`
  markers mid2agb turns back into a label and a GOTO. The song is written
  once through, not unrolled. What follows a looping GOTO up to the FINE
  is never played but is decoded too: mid2agb writes there what the MIDI
  holds after the loop's end (18 `ph_*_held` songs end a held note there).
- **Velocity and gate are written as stored** (multiples of 4, exact
  gate): mid2agb with `-E` maps them back unchanged.
- **Files:** `songs/<name>.mid`, `<name>` from the source map (pret's
  song header symbols, `mus_abandoned_ship`), else `song_<index>`.
  Songs whose header is the table's dummy (the header most entries
  share) are skipped; `mus_dummy`, with no tracks but its own header, is
  written, as a MIDI with no notes.
- **Result on Emerald (the songs unit, 2026-10-27):** the table at
  0x086B49F0, 610 entries, found from `mus_intro` played at frame 208;
  530 songs written; through mid2agb 492 of pret's 530 are exact (526
  with bar restarts, 530 with markers, 2026-10-28) and all 530 equal as events; pret's own `.s` assemble to the ROM's bytes for all
  530 (`crates/assets/src/mplay.rs`, the check's assembler).

VGMusicStudio's `SaveAsMIDI` is the model; its gaps (REPT, nested calls)
are handled here.

### Samples and voicegroups

Each song's voicegroup (`SongHeader.tone`) is a table of 12-byte
`ToneData`, read to the highest program any song using it selects:

- **Direct sound** (type 0, 8, 0x10): its `WaveData` to
  `samples/<name>.wav`: 8-bit mono, signed samples written unsigned, rate
  `freq / 1024`, a `smpl` chunk for the loop (status 0x4000, `loopStart`),
  and wav2agb's `agbp` (exact pitch) and `agbl` (loop end) chunks so it
  rebuilds byte for byte. A small RIFF writer of our own, since `hound`
  can't write custom chunks. DPCM (type 1, cries) is decoded first.
- **Programmable wave** (type 3, 0xB): its 16 bytes to `samples/<name>.pcm`,
  as pret keeps them.
- **Square and noise** (1, 2, 4, 9, 0xA, 0xC): parameters only.
- **Keysplits and drum sets** (0x40, 0x80): the nested voicegroup and key
  table.
- **The voicegroup itself:** `voicegroups/<name>.toml`, one entry a
  program with type, key, pan or sweep, ADSR and the sample's file.
- **A soundfont:** `soundfonts/<voicegroup>.sf2` for each song
  voicegroup, bank 0, one preset a program (its index), so a song's MIDI
  plays with its voicegroup's soundfont (the manifest gives each song's
  voicegroup and each voicegroup's file) in any player. Each direct-sound
  tone is a sample zone with its loop and root key 60 at `pitch / 1024`
  Hz (reverse tones reversed, fixed-rate ones unscaled), a keysplit one
  zone per run of keys, a drum set one zone per key at its tone's own key;
  ADSR converted to SF2 envelope units, and the PSG tones as short
  generated waves (a square cycle per duty, the 15- and 7-bit LFSR, the
  programmable wave). Not checked against pret (pret has no soundfont) and
  won't sound like the GBA's mixer exactly. **Not one `<game>.sf2`
  (finding):** with a bank a voicegroup Emerald needs 176 banks, past
  MIDI's 0–127 (and 128 means drums to most players), and its MIDI selects
  no bank.

**Result on Emerald (the samples unit, 2026-10-28):** 176 song
voicegroups and 15 nested ones (keysplits, drum sets), read to the
programs and keys their songs play; the two cry tables (`gCryTable`,
`gCryTable_Reverse`) found by shape, runs of 16 or more DPCM tones of one
type; 7 unused samples no tone reaches found right after ones that are
(each `WaveData` followed while the next has its shape). 544 `.wav`
(388 DPCM cries), all 544 rebuilt by wav2agb to pret's `.bin` and the
ROM's bytes; 21 of pret's 25 programmable waves (the other 4 no voice
uses) equal pret's.

- **Findings:** every `WaveData` holds one sample past its `size` (pret's
  `.wav`s hold it, `agbl` gives `size`), uncompressed and DPCM alike, so a
  WAV here holds `size + 1` samples and always an `agbl`; its `smpl` loop
  ends at the last sample, since wav2agb writes samples up to the loop's
  end. DPCM blocks are 33 bytes (a start sample, 64 steps high nibble
  first, the first 0), and wav2agb with lookahead 1 re-encodes a decoded
  cry exactly, since each target is one step away. The PSG tones keep
  their pan in the third byte (`length` in m4a's struct) and square 1 its
  sweep in the fourth.
- **The voicegroup TOML** lists the programs songs select (and for a cry
  table every entry), not every slot up to the highest.

## The manifest

`out/assets/<game>/manifest.toml`, the index of everything exported. Out
of git like all of `out/` (it is derived, and the assets are the game's,
private like the ROM). Keyed by ROM address, as the source map is, so the
same key names a blob in both.

```toml
[manifest]
rom_sha1 = "f3ae088181bf583e55daf962a92bb46f4f1d07b7"
routes = ["emerald-newgame"]
format = 1

[tiles]
0x08d9f8c0 = { store = "lz77", size = 3208, unpacked = 8192, bpp = 4,
               palette = 0x08d9fa00, palettes = [0x08d9fa00, 0x08d9fa20],
               use = ["bg"], files = ["tiles/gTitleScreenPokemonLogoGfx.png"],
               first_frame = 412, name = "gTitleScreenPokemonLogoGfx" }

[palettes]
0x08d9fa00 = { store = "raw", size = 32, banks = 1, files = ["palettes/gTitleScreenPokemonLogoPal.pal"] }

[tilemaps]
[sprites]   # w, h, mapping, frames = [offsets], animations = [{ tag, frames, durations }]
[songs]     # index, header, voicegroup, tracks, played_frame (if a route played it)
[samples]     # kind (pcm8, dpcm, wave), pitch, rate, size, looped, loop_start, file
[voicegroups] # kind (song, nested, cry_table), voices, songs, file, soundfont
```

- `name` is the source map's name for that address if it has one; the
  manifest stores it only for file names. The map stays the authority.
- **The other way round:** for a game with no decomp, the manifest gives
  the source map `[data]` entries it lacks: each blob's address, size and
  a kind-based name (`gfx_08d9f8c0`, `pal_08d9fa00`, `song_0012`), so code
  that loads a blob prints `addr!(gfx_08d9f8c0)` in the mod view. A
  `gbaa-assets names` step writes them as a map layer
  (`$GBAA_DATA/names/<game>-assets.toml`) under the mod view's layering
  rules (mod-view-design.md, "Where it lives"); any name a human gives
  later wins.

## Checking against pret

A new `gbaa-assets check-pret` reads the manifest and pokeemerald's tree
(sources, built tools and ELF), and writes
`out/assets/<game>/pret-check.txt`: counts per kind and a line per miss
with its reason.

### Joining to pret's files

- `pokeemerald.elf`'s symbols give each manifest address a pret symbol
  (or none: a range inside one, or noise).
- The `INCGFX` calls in pret's C (`INCGFX_U32("path.png", ".4bpp.lz",
  "-mwidth 2 -mheight 4")`) give each symbol its PNG, output kind and
  flags; `graphics_file_rules.mk`'s 83 extra targets (61 `cat`s, 21
  special flags) are parsed too, or listed as unjoined. A Rust parser in
  `crates/assets`, not a script (the owner's rule for check logic).
- `sound/song_table.inc` and `midi.cfg` give each song table index its
  `.mid` and flags; the voicegroup sources give each `WaveData` symbol its
  `.wav`.

### Graphics: four levels per pret image

| level | passes when |
|---|---|
| **bytes** | our decoded tile data equals `gbagfx <pret png> <out>.4bpp <flags>` (decompressed): the right blob, whole. |
| **rebuilds** | `gbagfx <our png> ... <flags>` then compression gives the ROM's bytes at the address: our file is an editable source. |
| **palette** | our paired palette's BGR555 values equal pret's (its `.pal` or PLTE, `x/8`). |
| **image** | our PNG's size and pixel indices equal pret's PNG. |

`image` fails where pret re-tiles for its own layout and we don't know it
(pret's 325 flagged calls; the sprite layout above covers the
`-mwidth`/`-mheight` ones whose sprites a route shows). The report gives,
for the route's blobs and for pret's whole graphics set: images matched at
each level, blobs found with no pret file, and pret files never shown
(coverage, not a miss). Tilemaps: byte-equal pret's `.bin`. Sprites'
animations have no pret file to match (pret keeps them in C tables); the
report lists their frame counts beside pret's frame strips.

### Music

- **Songs:** `mid2agb <our .mid> <out.s> <midi.cfg flags>` against pret's
  built `sound/songs/midi/<name>.s` (530, present after `make`), labels
  normalised. Equal means our MIDI rebuilds the ROM's song, since pret's
  `.s` assembles to the ROM's bytes. Where mid2agb factors patterns
  differently, compare the flattened per-track event streams (the decoder
  run on both), reported as a second level.
- **Samples:** `wav2agb -b <our .wav>` (`-b -c -l 1 --no-pad` for cries)
  against pret's `sound/direct_sound_samples/*.bin`, and the sample data
  against the ROM's `WaveData` bytes. Programmable waves: our `.pcm` equals
  pret's.
- Counts: songs exact, songs equal as events, samples exact, of 530, 530
  and 544.

## Overrides

A mod folder replaces assets when the game loads them, with no ROM edit.

### The mod folder

Under the mod view's mod copies, `mods/<game>/<mod>/` (mod-view-design.md,
"A mod copy"), an `assets/` folder and an `[assets]` table in `mod.toml`:

```toml
[assets]
0x08d9f8c0 = "assets/title-logo.png"          # tiles: PNG or .aseprite
0x08d9fa00 = "assets/title-logo.pal"          # a palette
0x08e2a5c4 = "assets/brendan-walking.aseprite"
```

The key is the manifest's address; the loader reads the manifest entry
for its layout (bpp, `w×h`, mapping, store) and encodes the edited file
back to the blob's unpacked bytes, the exporter's inverse, in
`crates/assets`. An Aseprite sprite takes its frames in their user-data
order; a palette `.pal` is `x/8` per channel. Mods are game-derived art,
so `mods/` stays out of git (`.gitignore`).

### In the runtime

The runtime (no_std, no alloc) is handed a sorted
`&'static [(u32 /* ROM address */, &'static [u8] /* unpacked bytes */)]`
by the desktop, which does the PNG and Aseprite decoding:

- **Compressed blobs:** the HLE LZ77 and RL calls look up their source
  address. On a hit they write the override's bytes to the destination
  (halfwords for the VRAM calls) instead of decoding. The override must
  be exactly the header's unpacked size, checked when the mod loads: the
  game sized its buffer from that header. The compressed size, the ROM
  space limit a ROM hack has, doesn't exist.
- **Raw blobs** (DMA'd frames, `CpuSet` palettes): the desktop gives the
  bus a copy of the ROM with those ranges replaced (same length), and the
  bus keeps the original ROM's CRC so states and saves stay
  interchangeable with unmodded play.
- **Off without a mod folder:** the table is empty, the lookup is one
  branch a BIOS call, and the ROM is the original, so play-view frame
  hashes are unchanged. `just check`'s routes prove it.
- **Both builds.** BIOS calls and DMA are runtime code in the interpreter
  and the recompiled build alike, so overrides of loads work in both. The
  exception: codegen's const-prop folds loads from ROM into recompiled
  code (`crates/codegen/src/func.rs:184`), so asset bytes the game's own
  code reads (not DMA or the BIOS) would keep their old value there. The
  overrides unit checks whether any manifest range is read by a folded
  load (codegen can list the ROM ranges it folded) and refuses such an
  override in a recompiled build until codegen leaves those ranges unfolded.
- **Music overrides** (an edited MIDI) need mid2agb's encoder in Rust.
  Not in this group; the manifest's addresses are ready for it.

**As built (assets/overrides, 2026-10-28):** `gbaa-desktop --assets-mod
DIR --assets-manifest MANIFEST` (both builds: the recompiled binary is
the same front end). `crates/assets/src/overrides.rs` encodes each
`[assets]` file against the manifest's layout (`bpp`, `width`, `frame`,
`layout`; an Aseprite file's frame size and count from `[sprites]`,
since a composed sprite's PNG may keep the plain layout) over the blob's
bytes decoded from the ROM, so a file of the wrong size, too many
frames' worth or an index past the bpp is refused when the mod loads;
tilemaps take their `.bin`. A layer-aware read: every visible layer's
cel, placed at its offset (Aseprite crops cels when it saves). The
desktop links `assets` without its `cli` feature, so no codegen. In
the runtime, `Bus::asset_overrides` is the sorted table; LZ77 and RL
decode as without the mod and then write the override over what they
wrote (bytes, or VRAM's halfwords), so registers, cycles and the
back-references' reads are unchanged and only the bytes differ.
`Bus::keep_rom_crc` gives the patched copy the original's CRC. The
folded ranges: codegen lists every load whose address const-prop knew
and the cartridge holds (`emit::folded`, exactly what it folds) into
`Code::folded` (Emerald: 22,839 merged ranges, all literal pools and
tables, none in an exported blob); a raw override over one is refused
in a recompiled run, which says to use `--interp`. A compressed blob's
override never is: the ROM's bytes stay. Check: `just
assets-check-overrides` (docs/tools/commands.md): all 106 exported
files round-trip; the new-game route with every file as an unedited mod
(91 blobs, 32 compressed, 59 raw) draws all 5,400 frames as plain play
in both builds, and the edited Brendan and truck tileset show at the
naming screen and the truck, the two builds' frames identical.

## Graphics coverage (june's call)

What a route doesn't show isn't extracted: the new-game route's 37
compressed blobs and 58 ranges are about 100 of pret's ~7,600 graphics.
Music has no such limit (the table gives every song). Ways to widen it,
asked of june as "How should asset extraction reach graphics a route
doesn't show?":

- **a. Routes only:** what the factory's routes show, a
  few hundred images with palettes, animations and layouts, all found by
  watching. Nothing more to build.
- **b. More short scripts** (the question's default): short input scripts from the crafted saves
  (the `emerald-save` tool) to screens with many assets: the bag, the
  party summary, the Pokédex, a wild battle, the PC. Still watching, a
  few thousand images, one small unit per screen.
- **c. A static scan as well:** every LZ77 header in the ROM that decodes
  cleanly (Nintenlord's rule) becomes a blob, greyscale unless a route
  drew it. Finds most of pret's 4,329 compressed files, but without
  palettes or layouts, and steps outside "watch where the data goes".

Before june answers, a CPU-heavy alternative was tried: fuzzing (random
presses from mutated saves and RAM, coverage-guided, no agent).
[fuzz-coverage.md](research/fuzz-coverage.md): 7.5 CPU-hours found about
1,200 image and palette symbols (the route: 94) and 47 screens the route
never shows, still climbing about 110 symbols a CPU-hour.

Only "Widen graphics coverage past what routes show"
(`assets/wider-coverage`) waits on the answer; the units below go ahead.

june's answer (2026-10-29): fuzzing, extended, aiming at ~90% of pret's
graphics that play uses, measured by a coverage report.

**As built (assets/coverage-report, 2026-10-29):** `just assets-coverage
[RUN]` (`gbaa-assets coverage`, `crates/assets/src/coverage.rs`,
docs/tools/commands.md). The denominator is pret's files: every `INCGFX`
as a source and output kind, 7,586 of them, joined to the ROM by the
source map's symbol (an aggregate's elements, as a tileset's sixteen
palettes, split its range evenly). Left out as never used in play, 181:
135 that pret names unused, 35 no aligned ROM word points into
(the Japanese contest screen, Ruby and Sapphire's running Brendan and
May, the secret bases' unused compressed tiles), 10 placeholder Pokémon
pictures (`SPECIES_NONE`'s and the cut old Unown's) and one built out.
That leaves 7,405: 3,875 images, 3,236 palettes, 294 tilemaps. A run is
scored by what reached the screen (a compressed blob's start, a copied
byte in the range) and, for palettes, the ROM loads too, since the
fades' colour maths loses the provenance tags on the way to palette RAM.
An image counts with its palette when a palette pret keeps in its folder
or by its name was found (a manifest says itself); "unpaired" when pret
keeps none beside it (Pokémon icons share `icon_palettes/`). The spike's
run rescored: the route phase 132 of 7,405 (1.8%); the whole run 1,609
(21.7%): 802 images (576 with palette, 132 greyscale, 94 unpaired), 734
palettes, 73 tilemaps; tilesets 78% of their images, item icons 48%,
Pokémon icons 36%, Pokémon pictures 9%, trainers 7%, contests and the
Battle Frontier none. The new-game route's export: 107 (1.4%).
Since assets/fuzz-to-90 (2026-10-30) 650 more are left out: the tileset
palette slots the map loader never copies (`LoadTilesetPalette` takes a
primary tileset's palettes 0-5 and a secondary's 6-12 of the 16 pret
keeps; the secret base's primary stays, since `decoration.c` reads any
of its slots), leaving 6,755 (2,586 palettes).
Since assets/coverage-unscripted-anim-tags (2026-10-31) 164 more are
left out: battle-animation sheets and palettes (fangs, fury swipes, the
Z, ...) whose `ANIM_TAG_*` no `loadspritegfx` in
`data/battle_anim_scripts.s` loads and no C loads by indexing
`gBattleAnimPicTable` (the intro's rocks, the gold stars, the Pokéblock,
the status impact): only the tag table points at them. That leaves
6,591 (3,784 images, 2,513 palettes); moves16's 3,333 found are 50.6%
of it (49.3% before), and battle-animation images 197 of 200. The
figures below are against 6,755.

**The curve (assets/fuzz-to-90, 2026-10-30).** Run with the story
seeds, the Pokédex and bag sweeps and v2's species, trainer and map
sweeps, scored in files by `coverage-curve.tsv` every 5 minutes. f90a
(15 workers, 55 min, sweeps a battle in five iterations and a map boot
in twenty) ended still climbing, 61.7% and about 125 files a 5 minutes,
its sweeps some 15% short. f90c (22 workers, 55 min, 20.6 CPU-hours,
sweeps a battle in three and a map boot in ten) finished its sweeps by
about 40 minutes and went flat: 31.3% at 5 min, 51.3% at 20, 61.6% at
35, 63.2% at 40, then 63.5, 63.8, 64.0 (12 files in the last 10
minutes). It ends at **4,325 of 6,755 (64.0%)**, 86.1% with the static
scan's greyscale sheets: footprints, item icons, tilesets, trainers and
Pokémon palettes 93-100%; Pokémon pictures 62%, icons 54%, battle
animations 53%, overworld objects 47%, menus and other screens 23%,
battle interface 15%, contests and the Frontier none. What random play
won't reach, and why: 412 still front pictures (`gMonStillFrontPicTable`,
read only by `pokemon_jump.c`, the wireless Pokémon Jump), with the
other link-only screens (Union Room, wireless status, Berry Crush,
Dodrio Berry Picking, Mystery Gift's cards and news, about 130 files);
about 280 battle animation files (a move's animation plays only when the
move is used, and a sweep battle's moves are rarely picked); 190 Pokémon
icons and 165 PC storage files (the PC's boxes, never opened with a full
box); 129 overworld objects (NPCs behind flags on maps a boot doesn't
show); the intro movie, Rayquaza's cutscene, the slot machine, roulette,
decorations, berries' tags, the PokéNav and contests, each a screen of
its own. Past 64% each takes a sweep that walks into it.

**Where it ends (assets/wider-coverage, 2026-10-30).** Emerald's final
counts against pret's 6,755 used graphics files: **4,325 found by play
(64.0%)**, 2,101 images with their palette, 94 greyscale, 134 unpaired;
**5,815 (86.1%) with the static scan's greyscale sheets**; 940 missing,
nearly all raw (uncompressed) data the scan can't see: menu palettes
333, overworld objects 129 and their palettes, Pokémon icons 190. The
~90% aim is not met by play: the curve is flat at 64% and the rest
needs a sweep per screen, filed as their own units, not this one's:
contest halls and Frontier facilities
(`assets/fuzz-contest-frontier-sweeps`), full PC boxes for Pokémon
icons (`assets/fuzz-pc-box-sweep`) and each move used in battle for
its animation (`assets/fuzz-move-sweep`). The link-only files (412
still front pictures, about 130 Union Room, wireless and Mystery Gift
files) stay in the denominator; without them play's share would be
about 70%. june kept them in (2026-11-01): the denominator stays
6,755 and the report is unchanged; reaching them waits on the
multiplayer link work. Rescore any later run with
`just assets-coverage out/fuzz/<run>`.

**PC boxes (assets/fuzz-pc-box-sweep, 2026-10-31).** Four PC passes
after the bag passes: every species and Unown form in the 14 boxes,
each box walked slot by slot on its own wallpaper, then its menus, move
items and Walda's wallpapers. With f90c's workers: **4,672 (69.2%)**,
89.9% with the scan; Pokémon icons 414 of 415, PC storage 132 of 165
found (32 scan only, one never loaded).
[fuzz-coverage.md](research/fuzz-coverage.md), PC box sweep.

**The static scan (assets/static-scan, 2026-10-29),** option c for the
last mile, june's call the same day: `just assets-scan [ROM] [OUT]
[RUN…]` (`gbaa-assets scan`, `crates/assets/src/scan.rs`) takes every
LZ77 blob by Nintenlord's rule (a word-aligned `0x10` header, 32 bytes
to 256 KiB unpacked and a multiple of 4, a clean decode inside the ROM
with no back-reference before its start; a blob found is skipped over)
and writes the ones no RUN reached as greyscale 4bpp sheets,
`OUT/scan/lz_<addr>.png`, marked "found by scan, no palette/layout" in
the PNG's text and `OUT/scan/scan.toml`. No pret needed. In Emerald it
finds 4,350 blobs: 4,332 at a source map symbol's start, 10 real
payloads inside other symbols (`sDuplicateGfx`'s, the multiboot
programs'), 8 false ones in raw data; all 4,245 of the used LZ77 files
(the 12 run-length ones it doesn't look for). The spike's run reached
743; 3,607 are left for the sheets (5.4 MiB unpacked). `just
assets-coverage` scores with it as a "scan only" column, a file the run
missed whose blob the scan finds, not counted as found: the spike's
run 1,609 found (21.7%), 5,119 (69.1%) with the scan's sheets (3,510
scan only: Pokémon pictures 1,126, battle animations 263, item icons
113 and their palettes, most menus' tilemaps). Left missing are the
uncompressed files (tileset palettes 835, Pokémon footprints 385 and
icons 266, overworld objects): the scan can't see raw data.

**Kirby & The Amazing Mirror (assets/kirby-fuzz, 2026-10-31).** The
fuzzer ported to a game with no pret (`just kirby-fuzz`; the shared part
is `crates/fuzz/src/core.rs`, Kirby's sweeps `kirby.rs`), scored only
against the static scan's 1,202 LZ77 blobs: **458 (38.1%)** in 9
CPU-hours, flat after the 5-minute room sweep (a door to each of the 983
rooms). The rooms' graphics are all reached; the sub-games, cutscenes,
link screens and other languages' text are not, each behind a menu or
setting a sweep must open. A screen sweep then opened them
(assets/kirby-fuzz-screens): the game's own fade to each screen's
creator (sub-games, intro, endings, menus, the collection room, the
pause help per ability) in each of the six languages, `gLanguage`
patched in a ROM copy: **1,044 (86.9%)**, nearly all in three minutes.
[fuzz-coverage.md](research/fuzz-coverage.md), Kirby.

## Units

| unit | what | needs |
|---|---|---|
| Load and use logs for asset export (`assets/load-log`) | the load log and use log above, `just assets-record`; check: on the new-game route every one of the spike's 37 blobs and 58 ranges is in the load log with its full length, and every BG and OBJ tile shown at the truck has a use record | this |
| Export Emerald's tile sheets, palettes and tilemaps (`assets/graphics-export`) | `crates/assets`, tile sheets, palettes, tilemaps, the manifest and `names`, `check-pret` for graphics | load-log |
| Sprites from OAM with animations to Aseprite (`assets/sprites`) | sprite assembly, frames, animation timing and tags, composed sprites, the Aseprite writer; check: pret's object-event and Pokémon strips the route shows match at `image`; every file round-trips | graphics-export |
| Export Emerald's songs to MIDI (`assets/music-export`) | the m4a recorder, the song table scan, songs to MIDI, `check-pret` for songs | this |
| Emerald's samples, voicegroups and a soundfont (`assets/music-samples`) | samples to WAV and `.pcm`, voicegroup TOML, the SF2, `check-pret` for samples | music-export |
| Asset overrides: the runtime serves edited assets (`assets/overrides`) | as above; check: an edited sprite (Aseprite) and tile sheet (PNG) show in game, an unedited export round-trips to identical frames, play-view hashes unchanged | sprites |
| june edits an Emerald sprite in Aseprite and sees it in game (`assets/sprite-mod-gate`) | june's gate, playing songs with the soundfont | overrides, music-samples |
| Extract-while-playing flag in the desktop app (`assets/play-extract`) | `--extract DIR` and `just play-extract`, above; provenance in the recompiled build and its cost | graphics-export |
| Widen graphics coverage past what routes show (`assets/wider-coverage`) | widen graphics coverage the way june picks in "Graphics coverage" above; report the new counts against pret | graphics-export, june's answer |
