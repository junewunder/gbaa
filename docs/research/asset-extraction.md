# GBA asset formats and rippers

The literature review behind asset extraction (decision 2, "Two views of
a ROM", june 2026-10-26). It is unit `assets/lit-formats`, for
`assets/design`, alongside the provenance spike (`assets/provenance`).
The goal: get graphics and music out of a ROM without reading its code by
hand, by watching where the game's data goes at run time. Emerald comes
first, and pret's pokeemerald (graphics as PNG, music as MIDI) is the
oracle. The questions: what the hardware's formats are, what pret's files
are and how they're built, how existing rippers find assets, what earlier
work tracked data's origin inside an emulator, how emulators key asset
replacements, and what we'd write the files with.

Researched 2026-10-26 on quack. pokeemerald was read and measured in the
local clone and build (`$GBAA_DATA/src/pokeemerald`, commit `731ad5bf`,
set up by [discovery-prior-art.md](discovery-prior-art.md)). The tools
were read in their source, shallow-cloned at the commits in "Sources".
GBATEK was fetched 2026-10-26; the papers were read as their PDFs except
where marked. Three subagents did the first pass (music, graphics,
Aseprite and prior work). Their counts and pointers were spot-checked
against the clones: the m4a struct offsets against
`constants/m4a_constants.inc`, the song table address against
`pokeemerald.map`, agbplay's scan constants, gbagfx's `minDistance`,
aseprite-io's writer, PPSSPP's `[indexed]` section and `png` in our
`Cargo.lock`. Every source is also in [references.md](../references.md).

## Summary

- **Emerald needs only LZ77, RL and plain copies.** pokeemerald's 7,608
  `INCGFX` calls produce `.lz` outputs (4,329), `.rl` (12) and
  uncompressed tiles and palettes. None produce Huffman or Diff output,
  the two calls our HLE BIOS doesn't have (decision 12).
- **Our BIOS is ours, so its decompressors are the natural hook.** A
  BIOS `LZ77UnComp` or `RLUnComp` call in `runtime::bios::decomp`
  (`crates/runtime/src/bios/decomp.rs:169`, `:249`) knows the ROM source,
  the header (type and size) and the destination range. That is a
  provenance record for free, with no per-byte taint. mGBA, by contrast,
  loses this hook when a real BIOS image is loaded. DMA
  (`crates/runtime/src/dma.rs`) and `CpuSet`/`CpuFastSet` (`decomp.rs:42`,
  `:94`) are the same shape: a source range copied to a destination
  range. Only bytes the CPU writes itself need per-byte tags.
- **pret's graphics are indexed PNGs, and the build is byte-exact.**
  3,524 of 3,850 PNGs are 4-bit indexed with a 16-colour palette. No
  `.lz`, `.4bpp` or `.gbapal` is committed, yet `make compare` matches the
  ROM's SHA-1. So gbagfx's greedy LZ77 compressor reproduces Nintendo's
  compressed bytes for Emerald.
  - The strong check for a recovered image: run it back through gbagfx
    with the same `INCGFX` flags, and compare the bytes with the ROM at
    the source address the provenance records.
  - The weak check: compare the pixel indices with pret's PNG. 58 PNGs
    are greyscale without a palette, and 325 `INCGFX` calls re-tile
    (`-mwidth`/`-mheight`) or trim (`-num_tiles`), so pret's PNG layout
    is not always VRAM's.
- **pret's music is 530 MIDI files with no hand-written songs left.**
  Songs are built with `mid2agb` (all with `-E`, exact gate time) and
  samples with `wav2agb`, also byte-exact.
  - mid2agb is lossy (24 clocks a beat, velocity quantised to steps of
    4, only some controllers kept), so our MIDI can't equal pret's byte
    for byte.
  - The strong check is the same round trip: our MIDI → pret's mid2agb
    (with the song's `midi.cfg` flags) → bytes, compared with the ROM.
    Where pattern factoring or running status differ, compare the
    flattened per-track event streams.
- **m4a can be found at run time from one fixed address.** `SoundInit`
  writes `&gSoundInfo` to `0x03007FF0`, and its `ident` is `'Smsh'`
  (0x68736D53, or one more while locked). `SoundInfo.musicPlayerHead`
  (+0x24) links every `MusicPlayerInfo`, whose `songHeader` (+0x00) is the
  playing song. A recorder that reads this once a frame sees every song
  as it starts, with no Emerald address.
  - The song table itself comes from a ROM scan for `{header, ms, me}`
    entries with `ms == me` (agbplay's rule), or from the word 40 bytes
    after the `m4aSongNumStart` code signature (gba-mus-ripper). Both
    find 0x086B49F0 and 610 entries in Emerald.
- **No ripper does what we want, but VGMusicStudio is the template for
  MIDI.** agbplay renders WAV only. gba-mus-ripper writes format-0 MIDI
  and an SF2, and drops REPT, MEMACC, PRIO and XCMD. VGMusicStudio writes
  format-1 MIDI at 24 ticks a beat with the controller numbers mid2agb
  reads, so it is close to an inverse of mid2agb. It finds songs from a
  hard-coded per-game table, not a scan.
- **No emulator records where graphics came from.** mGBA's viewers export
  PNGs and only mark tiles dirty. Its access logger flags bytes as
  written by DMA, decompression or copy, but keeps no source address.
  Mesen2's GBA code/data logger marks ROM bytes read as data, but GBA has
  no "drawn" flag, because the PPU reads VRAM, not ROM. We found no paper
  on extracting sprites from emulator memory, and no GBA emulator with
  tile replacement.
- **Whole-system taint costs 2–20×** (TaintBochs 2–10×, DECAF ~6×,
  Panorama ~20×, libdft 1.1–6×), with a source label per byte. Our case
  is narrower: the sources we care about are copies, DMA and BIOS
  decompression, which are range operations. CPU-drawn bytes are the
  only per-byte case, and the spike measures how many there are.
- **Replacement packs key on content, not address.** Dolphin, GLideN64,
  Citra and PPSSPP hash the texture bytes and, separately, the palette
  (Dolphin over only the indices used), with wildcards so one image
  serves every palette. PPSSPP's `[indexed]` images and Mesen's
  default-palette tiles do the same. For a GBA, key tiles by their 4bpp
  bytes, palettes apart, and keep the edited image indexed. A ROM
  address from provenance is the readable name; the content hash is
  what the runtime matches.
- **Write Aseprite natively, in Rust.** `aseprite-io` 0.2.0 (MIT/Apache)
  reads and writes indexed files with frames, durations, tags and user
  data, and round-trips bytes. It is young (created 2026-03). Aseprite's
  own CLI isn't installed and its licence forbids redistribution, so
  nothing can depend on it. `png` (already in `Cargo.lock`) writes
  4-bit indexed PNGs. `midly` and `hound` write MIDI and WAV; neither
  is in the lock file yet.

## The hardware's formats

From GBATEK's LCD sections, cross-checked against mGBA's
`include/mgba/internal/gba/video.h` and `src/gba/renderers/`. Our PPU
(`crates/runtime/src/ppu.rs`) draws all of these.

- **Tiles** ("LCD VRAM Character Data"): 8×8. 4bpp is 32 bytes, rows top
  down, low nibble the left pixel; 8bpp is 64 bytes, a byte a pixel.
- **Palette RAM** ("LCD Color Palettes"): BG at 0x05000000, OBJ at
  0x05000200, each 16 banks of 16 colours or one of 256. BGR555 (red in
  bits 0–4). Colour 0 of each bank is transparent; BG colour 0 is the
  backdrop.
- **Backgrounds** ("LCD I/O BG Control", "LCD VRAM BG Screen Data
  Format"): `BGxCNT` gives the character base (bits 2–3, 16 KB units),
  8bpp (bit 7), screen base (bits 8–12, 2 KB units) and size (14–15).
  - A text map entry is 16 bits: tile 0–9, H-flip 10, V-flip 11, palette
    bank 12–15. Maps larger than 32×32 are 32×32 blocks of 0x800 bytes.
  - An affine map entry is 8 bits, tile 0–255, always 8bpp, no flips.
  - Modes: 0 is four text BGs; 1 is two text and BG2 affine; 2 is BG2
    and BG3 affine; 3–5 are bitmaps on BG2 (mode 3 240×160×16 bits,
    mode 4 240×160×8 bits with two frames at 0x06000000 and 0x0600A000,
    mode 5 160×128×16 bits, two frames).
- **Sprites** ("LCD OBJ - OAM Attributes"): 128 entries of 8 bytes.
  - attr0: Y, affine, double-size or disable, mode, mosaic, 8bpp, shape.
    attr1: X, affine index or H/V flip, size. attr2: tile, priority,
    palette bank. PA–PD are each entry's fourth halfword (32 groups).
  - Shape × size gives 8×8 to 64×64 (`GBAVideoObjSizes`,
    `src/gba/video.c:40`).
  - OBJ tiles start at 0x06010000; in modes 3–5 only 0x06014000 up (tile
    numbers 512 up) are shown.
  - `DISPCNT` bit 6 picks 1D mapping (a sprite's tiles in a row) or 2D
    (a 32-tile stride, `software-obj.c:168–212`). An exporter assembling
    a sprite from OAM has to follow it.

## BIOS compression

GBATEK "BIOS Decompression Functions" and "BIOS Memory Copy"; mGBA's HLE
in `src/gba/bios.c`.

- **Header word:** type in bits 4–7 (1 LZ77, 2 Huffman, 3 RL, 8 Diff),
  size in bits 8–31. Huffman's bits 0–3 are bits a symbol (4 or 8);
  Diff's are the unit (1 or 2 bytes).
- **LZ77** (`0x11` WRAM, `0x12` VRAM): a flag byte, MSB first, before each
  8 blocks; a set bit is a 2-byte back-reference copying N+3 bytes from
  `dest − disp − 1`. The VRAM call writes halfwords, so it can't use
  `disp` 0. mGBA: `_unLz77` (`bios.c:628`).
- **Huffman** (`0x13`, `_unHuffman`, `:721`), **RL** (`0x14`/`0x15`,
  `_unRl`, `:793`), **Diff** (`0x16`–`0x18`, `_unFilter`, `:869`),
  **BitUnPack** (`0x10`, `_unBitPack`, `:914`). mGBA's HLE versions tag
  their writes `mACCESS_DECOMPRESS`. `CpuSet`/`CpuFastSet` run as ARM
  code in mGBA's `hle-bios.s` (`:199`, `:252`), not in C.
- **Ours:** decision 12's HLE has LZ77, RL, BitUnPack, `CpuSet` and
  `CpuFastSet`, but not Huffman or Diff. Emerald doesn't need them
  (above). Another game might, and its first call faults with the
  number, as decision 12 says.

## pokeemerald's graphics: the oracle

Measured with `git ls-files` at `731ad5bf`.

- **Files:** `graphics/` tracks 5,728 files: 3,850 PNG, 1,441 `.pal`
  (JASC) and 435 `.bin` tilemaps. The largest folders are pokemon
  (2,853), items (471), battle_anims (404), object_events (289),
  pokemon_storage (139), pokenav (112) and trainers (107). Map tilesets
  live in `data/tilesets/` (226 PNG, 1,200 `.pal`, 140 `.bin`). The
  882 `.bin` files in `data/layouts/` are metatile grids, not GBA maps.
- **PNG kinds:** 3,524 are 4-bit indexed with a 16-colour PLTE, 391 are
  1-bit (footprints), 58 are 4-bit greyscale with no palette, and the
  rest 8-bit indexed.
- **How they're built:** each C file names its asset inline, e.g.
  `INCGFX_U32("path.png", ".4bpp.lz", "-mwidth 2 -mheight 4")`.
  `tools/scaninc` makes the rules and `tools/preproc/c_file.cpp:458`
  inlines the bytes. There are 7,608 calls over 6,947 sources. Outputs:
  `.4bpp.lz` 2,355, `.gbapal` 1,741, `.gbapal.lz` 1,591, `.4bpp` 1,121,
  `.1bpp` 391, `.lz` 368, `.8bpp.lz` 15, `.rl` 12, fonts 12.
  - 325 calls carry flags: 226 `-mwidth`/`-mheight` (re-tiling into
    metasprite order), 94 `-num_tiles` (trim), 4 `-plain -data_width`.
  - `graphics_file_rules.mk` has 83 targets more: 61 `cat`s (several
    PNGs into one blob, e.g. Castform's forms) and 21 with special flags.
- **Layouts an exporter must match:**
  - Pokémon: `front.png` 64×64; `anim_front.png` 64×128 (two frames
    stacked); `back.png` 64×64; `icon.png` 32×64 (two frames);
    `normal.pal` and `shiny.pal` beside them.
  - Object events: a horizontal strip of frames, e.g. Brendan's
    `walking.png` is 144×32, nine 16×32 frames, re-tiled with
    `-mwidth 2 -mheight 4` (`overworld_frame`, `include/sprite.h:34`).
  - Tilemaps: raw 16-bit text entries (2,048 bytes for 32×32, also 1,280,
    4,096 and 8,192), some LZ-compressed.

### gbagfx

`tools/gbagfx` (`main.c:600–622`): 1/4/8bpp ↔ PNG, PNG → `.gbapal` or
JASC `.pal`, fonts, and LZ, RL and Huffman both ways.

- Tile data uses only the PNG's indices; its PLTE matters only to the
  palette rules. Without `-palette`, gba → PNG writes greyscale.
- Colours convert as `x*255/31` and back as `x/8`, which round-trips.
- The LZ compressor (`lz.c:72–153`) is greedy: nearest distance first,
  first longest match, length capped at 18, padded to 4 bytes,
  `minDistance` 2 by default (`main.c:431`, safe for the VRAM call). It
  matches Nintendo's output for Emerald (the SHA-1 says so; no `.lz` is
  committed). `-overflow` and `-search` exist for other games; Emerald
  uses neither.

## Music: m4a

Nintendo's MusicPlayer2000 ("Sappy" to rippers). pret's source:
`src/m4a.c`, `src/m4a_1.s`, `include/gba/m4a_internal.h`,
`constants/m4a_constants.inc`.

### Structures

- **`Song`** (`m4a_internal.h:360`): `header*`, `u16 ms`, `u16 me`
  (player numbers), 8 bytes. `gSongTable` is at 0x086B49F0, right after
  `gMPlayTable` (0x086B49C0: 4 players × 12 bytes; BGM 10 tracks, SE1 3,
  SE2 9, SE3 1).
- **`SongHeader`** (`:223`): `trackCount, blockCount, priority, reverb,
  tone*` (the voicegroup), then a pointer to each track's bytecode.
- **`ToneData`** (`:57`, 12 bytes): `type, key, length, pan_sweep, wav*,
  attack, decay, sustain, release`. Types (`asm/macros/music_voice.inc`):
  direct sound 0x00 (with `no_resample` 0x08, `reverse` 0x10,
  `compressed` 0x20); square 1/2, wave, noise 1–4 (+8 for the alt
  forms); keysplit 0x40 (a voicegroup and a key table); drum set 0x80
  (a voicegroup indexed by key).
- **`WaveData`** (`:39`): `u16 type` (0 PCM, 1 DPCM, used by Pokémon
  cries), `u16 status` (0x4000 loops), `freq` (pitch × 1024),
  `loopStart`, `size`, then signed 8-bit samples.
- **`MusicPlayerInfo`** (0x40 bytes, offsets from `m4a_constants.inc`):
  `songHeader` +0x00, `status` +0x04 (low 16 bits the active tracks, bit
  31 paused), `trackCount` +0x08, `tracks` +0x2C, `tone` +0x30,
  `ident` +0x34, `musicPlayerNext` +0x3C. A `MusicPlayerTrack` is 0x50
  bytes, with `cmdPtr` at +0x40.
- **`SoundInfo`**: `ident` +0x00, `MPlayMainHead` +0x20,
  `musicPlayerHead` +0x24. Emerald's is `gSoundInfo` at 0x03006380.

### Bytecode

`sound/MPlayDef.s`, decoded by `MPlayMain` (`m4a_1.s:1232–1260`):

- 0x00–0x7F are arguments, with running status (latched only for
  commands 0xBD up).
- 0x80–0xB0 wait (W00–W96, through `gClockTable`).
- 0xB1–0xCE are commands through `SoundInfo->MPlayJumpTable`: FINE B1,
  GOTO B2, PATT B3, PEND B4, REPT B5, MEMACC B9, PRIO BA, TEMPO BB (BPM
  ÷ 2), KEYSH BC, VOICE BD, VOL BE, PAN BF, BEND C0, BENDR C1, LFOS C2,
  LFODL C3, MOD C4, MODT C5, TUNE C8, XCMD CD, EOT CE.
- 0xCF is TIE and 0xD0–0xFF are notes N01–N96.

### Finding it at run time

- `m4aSoundInit` (`m4a.c:70`) copies the mixer into IWRAM and calls
  `SoundInit` (`:353`), which stores `&gSoundInfo` at 0x03007FF0
  (`include/gba/defines.h:22`) and sets `ident` to 0x68736D53 last. While
  a critical section runs, `ident` is one more (`m4a_1.s:1070–1080`).
- `MPlayOpen` (`m4a.c:565`) pushes each player onto
  `musicPlayerHead`; `MPlayStart` (`:611`) sets `songHeader` and each
  track's `cmdPtr`. `m4aSongNumStart` (`:107`) is only
  `MPlayStart(gMPlayTable[ms].info, header)`.
- So a recorder can read 0x03007FF0 once a frame, check `ident`, walk the
  player list and note each new `songHeader`, and that is every song
  played, by ROM address. Cries are the exception: their header is
  copied into RAM (`gPokemonCrySong`), so their `songHeader` points to
  RAM.
- The whole table (songs never played on a route) needs a ROM scan:
  - **agbplay** (`MP2KScanner.cpp:101–162`): word-aligned from 0x200,
    at least 4 consecutive valid entries (`MIN_SONG_NUM`): a ROM header
    pointer, `ms == me`, padding bytes zero, a valid voicegroup and
    track pointers (`IsValidSongTableEntry`, `:641`), and the table's
    start referenced by some ROM word.
  - **gba-mus-ripper** (`sappy_detector.c:35–49`): two 30-byte Thumb
    signatures of `m4aSongNumStart`; the song table is the word at +40.
    Then it scans back for `push {lr}` and checks the sound mode, player
    count and player table 16 bytes before ("most games") or 32
    ("Pokémon").
  - On Emerald both give 0x086B49F0. The music subagent ran both rules
    on the ROM, and agbplay's walk gives exactly 610 entries.

### pokeemerald's sound/: the oracle

- `song_table.inc`: 610 entries; 530 real songs, 80 pointing at
  `dummy_song_header`.
- `sound/songs/midi/`: 530 `.mid` (210 `mus_`, 269 `se_`, 51 `ph_`) and
  `midi.cfg`. No hand-written `.s` song is left; the last were converted
  in 2026-03 (commit `1b35dd881`). Format 1, 24 ticks a beat.
- 195 voicegroups (`sound/voicegroups/`, with `drumsets/` and
  `keysplits/`), 5 keysplit tables, 2 cry tables of 388.
- 544 direct-sound `.wav` (105 instruments, 388 cries, 51 phonemes),
  8-bit mono; 25 programmable wave `.pcm` of 16 bytes (32 4-bit samples).
- Rules (`audio_rules.mk:22–45`): `mid2agb` with each song's `midi.cfg`
  options; `wav2agb -b` for samples and `-b -c -l 1 --no-pad` for cries
  (DPCM).

### pret's converters

- **mid2agb** (`tools/mid2agb`, MIT): rescales to 24 clocks a beat
  (`midi.cpp:635`), quantises velocity to steps of 4 (`:639`), snaps
  lengths to the note table unless `-E` (`:646`), drops SysEx,
  aftertouch, most meta events and unread controllers. It reads CCs 1,
  7, 10, 12–17, 20–22, 24, 26, 29–31, 33 and 39 as m4a commands
  (`agb.cpp:349–414`), and finds PATT patterns itself.
- **wav2agb** (`tools/wav2agb`, MIT, by ipatix): `.s` or `.bin`; `-c`
  DPCM. Custom RIFF chunks `agbp` (exact pitch) and `agbl` (loop end)
  let vanilla samples rebuild byte-exact.
- **ipatix's midi2agb** is a clean rewrite whose README says it doesn't
  match mid2agb's output, so it can't be our check.

### Rippers

| Tool | Finds songs by | Writes | Drops |
|---|---|---|---|
| agbplay (LGPL-3.0) | ROM scan (above) | rendered WAV, per song or track | (no MIDI) |
| gba-mus-ripper (no licence file) | `m4aSongNumStart` signature | format-0 MIDI, SF2 | REPT, MEMACC, PRIO, XCMD |
| saptapper (LGPL-3.0) | same signature | GSF | — |
| VGMusicStudio (LGPL-3.0) | hard-coded table per game (Emerald 0x6B49F0) | format-1 MIDI at 24 ticks, WAV | REPT, nested calls (its TODOs, `Player.cs:642`) |

The music subagent found no standalone "sappy2midi" or "agb2mid" (the
repos named so return 404), and no published comparison of ripped MIDI
with pret's.

## Emulator viewers and loggers

- **mGBA** (`src/platform/qt/`): TileView, ObjView, MapView and
  PaletteView. Tiles and sprites export as PNG (a sprite as indexed with
  its real palette, `AssetView.cpp:136–160`), maps as RGB, palettes as
  RIFF `.pal` or `.act`. VRAM writes only mark tiles dirty
  (`tile-cache.c:74`). The access logger (`src/debugger/access-logger.c`)
  flags each byte read, written, or touched by program, DMA, decompress
  or copy, with no source address.
- **Code/data loggers** mark ROM bytes by use:
  - FCEUX: code, data, PCM, and for CHR "rendered".
  - Mesen2: has a GBA core. Its logger marks code, data and Thumb
    (`GbaCodeDataLogger.h`), and DMA reads count as data, but there's no
    "drawn" flag.
  - BizHawk: no GBA logger.
  ROM hackers diff these logs between runs to find what a scene uses.
  For us, the provenance map plays the logger's part, with the
  destination added.
- **NO$GBA:** its VRAM viewer shows tiles, maps, OAM and palettes with
  addresses; screenshot export only (not verified further).
- **Static rippers** scan the ROM for LZ77 headers (type 0x10, a
  plausible size, a dry-run decode with no reference before the start:
  Nintenlord's `LZ77.cs:61–80`), or read per-game tables (HexManiacAdvance,
  PokemonGameEditor). Tile Molester shows raw tiles. None know which
  palette a tile is drawn with, which is what run time adds.

## Tracking where data came from

| System | Granularity | Cost |
|---|---|---|
| TaintBochs (USENIX Sec 2004) | byte, RAM and registers | 2–10× Bochs (§6) |
| TEMU / Panorama (CCS 2007) | byte, with a source record | ~20× (§5.3) |
| DECAF (ISSTA 2014) | bit, on QEMU TCG | 6× average on SPEC2006 (Table 1) |
| libdft (VEE 2012) | 1 bit or 1 byte a byte, one process | 1.1–6× |
| Dytan (ISSTA 2007) | byte, configurable | ~30× (quoted by libdft; paper not read) |
| PANDA taint2 | byte, a label set each (`shad.h:83`) | "~10× faster" than taint1 (README; paper not read) |

All of them propagate taint through every instruction. Decompression
is the hard case for them: LZ77 in software is table lookups and
arithmetic, and plain copy taint loses the source. TaintBochs added
address-taint rules for this. We can skip most of that: the BIOS
decompressors, DMA and `CpuSet` are single calls in our runtime with a
known source range, so they can record `(source, transform, destination
range)` and stamp the destination's shadow directly. Per-instruction
tags are needed only for a game's own decompressor or CPU copy loops.
How much of Emerald's VRAM those cover is the provenance spike's
question.

A shadow of VRAM (96 KB), palette (1 KB) and OAM (1 KB) with a 4-byte
source tag each is about 400 KB. Shadowing all RAM as well (EWRAM 256 KB
and IWRAM 32 KB) would be about 1.2 MB more. The runtime already has the
pattern for zero-cost-when-off instrumentation: the `trace` feature
(`crates/runtime/src/trace.rs`, `Cargo.toml` `[features]`).

Related academic work is about levels, not assets. VGLC (Summerville et
al., PCG 2016) is a corpus of levels. Guzdial and Riedl (AIIDE 2016)
parse gameplay video against a fan-made spritesheet. Mappy (Osborn et
al., FDG 2017) and MappyLand (AIIDE 2021, Rust, ~1 ms a frame) map NES
games from an instrumented emulator's nametables and OAM: the nearest
thing to ours, for maps. The Aseprite subagent found no work on
extracting sprites by provenance.

## Replacing assets: how emulators key them

| Emulator | Key | Palette |
|---|---|---|
| Dolphin (`TextureInfo.cpp:113–175`) | `tex1_{w}x{h}_{xxh64(data)}_{xxh64(tlut)}_{fmt}` | hash over only the indices used; `$` wildcard |
| PPSSPP (`TextureReplacer.cpp`) | address and CLUT hash, then a data hash | `[indexed]` images (`:466`) serve every palette |
| Citra/Azahar | CityHash64 of the upload or the decoded RGBA | in the data |
| GLideN64 (`TxHiResLoader.cpp:93–131`) | `ROM#CRC32#fmt#siz[#PALCRC]` | `$` wildcard |
| Mesen HD packs (NES) | CHR-ROM: tile index; CHR-RAM: the 16 tile bytes; plus 4 palette bytes | "default tile" matches any palette |

The common lessons: key on content, not address, since addresses move
when data is decompressed or streamed. Keep the palette a separate,
optional key, and store edited images as indices so palette swaps share
one file. A ROM address is still the right name in a manifest the source
map can label. It just isn't what the runtime matches on.

## Writing the files

- **Aseprite's format** (`docs/ase-file-specs.md`, aseprite `b3e34ac`;
  last changed 2026-04-20; release v1.3.18.6):
  - A 128-byte header: magic 0xA5E0, frames, size, depth 8 for indexed,
    and the transparent index (for non-background layers).
  - Each frame: magic 0xF1FA and a duration in ms.
  - Palette: chunk 0x2019; old files may also have 0x0004, so readers
    handle both.
  - Layer 0x2004. Cel 0x2005: type 2 is zlib-compressed indices, type 1
    links to another frame's cel.
  - Tags 0x2018 (frame range, direction, repeat): GBA animations.
  - User data 0x2020 attaches to the chunk before it, with typed
    properties. That is where a ROM address and palette bank would go,
    so an edited file can be read back to its source.
- **Crates:**

  | Crate | Version | Writes | Indexed | Notes |
  |---|---|---|---|---|
  | aseprite-io | 0.2.0 (2026-06-30) | yes (`writer.rs:145`) | yes | MIT/Apache; byte-exact round trip; young (5 stars) |
  | asefile | 0.3.8 (2024-03) | no | reads, composites to RGBA | MIT; most used |
  | aseprite-loader | 0.4.2 (2026-02) | no | converts to RGBA | MIT/Apache |
  | png | 0.18.1 | yes | 4-bit PLTE + tRNS | in `Cargo.lock` already |
  | midly | 0.5.3 | yes (`Smf::write`) | — | Unlicense; no_std with `alloc` |
  | hound | 3.5.1 | yes | — | Apache-2.0 |

- **Aseprite itself** isn't installed. Its EULA allows building it only
  for personal use and forbids redistribution, so the pipeline writes
  the files itself and june opens them.

## What we take

For the provenance spike (`assets/provenance`):

- Hook the range operations first: BIOS LZ77, RL, BitUnPack, `CpuSet`
  and `CpuFastSet`, and DMA. Each records source, transform and
  destination range, and stamps a shadow of VRAM, palette and OAM. CPU
  stores into those regions get a "computed" tag, or the source of the
  value if registers carry tags; whether that's worth it is what the
  spike measures.
- Bytes often reach VRAM in two steps: decompressed into EWRAM, then
  DMA'd or copied to VRAM. So the shadow should cover EWRAM too, or the
  second copy loses the source.
- For music, read m4a's state from 0x03007FF0 each frame rather than
  tagging the sound FIFOs. The FIFOs carry mixed samples, and their
  source is the mixer.

For the design (`assets/design`):

- **Graphics check:** re-encode a recovered tile blob with gbagfx (the
  `INCGFX` flags pret uses for that address, from the ELF symbols) and
  compare with the ROM bytes at its source. Then compare its pixels with
  pret's PNG after pret's re-tiling. Count exact matches per pret file.
- **Music check:** our MIDI through pret's mid2agb with `midi.cfg`'s flags,
  compared with the ROM's song bytes; flattened event streams where
  pattern factoring differs. VGMusicStudio's `SaveAsMIDI` is the model
  for the controller mapping, and its gaps (REPT, nested PATT) are ours
  to close.
- **Formats:** indexed PNG (`png`), Aseprite (`aseprite-io`, or our own
  small writer if it proves unreliable; the format is simple), MIDI
  (`midly`), WAV (`hound`). Provenance goes in each file's user data and
  in the manifest.
- **Overrides:** match by content hash of the tile bytes, palette apart
  and optional, images kept indexed (Dolphin, PPSSPP, Mesen).

## Sources

| Source | Commit or version |
|---|---|
| pret/pokeemerald (local build) | `731ad5bfd6e6f265508d0efcca0ba42f9dcf5881` |
| GBATEK, problemkaputt.de/gbatek.htm | fetched 2026-10-26 (no version) |
| mgba-emu/mgba | `1509d86028c7df2d844b590723a80b2978f49e7c` |
| ipatix/agbplay | `0b87da48d2502da359e45718eec8566ac40fa9d7` |
| berg8793/gba-mus-ripper (Bregalad's GBA Mus Ripper) | `b30a5d932a9892deb496030960032eaed185074f` |
| loveemu/saptapper | `ff7ec3e4da1f1ffc3bcc05793268036a319b4466` |
| Kermalis/VGMusicStudio | `d14a38e264eeb1392289eeb2bd5b450c92877949` |
| ipatix/midi2agb | `19a6f83f94af9efddc764dfe0b2e3dd86bf25e96` |
| TimoVesalainen/Nintenlord.Hacking | `aa802ac407fc6e26aba8d154cc53d1278dcf3046` |
| haven1433/HexManiacAdvance | `c8de1e8acf753522af7feed810d6324ce52474a3` |
| Gamer2020/PokemonGameEditor | `441ff797499bd0e9b6c18f0f7493185ca001e908` |
| toruzz/TileMolester | `50fb208e63f414fdd35e951fec6cad1f3230e056` |
| aseprite/aseprite | `b3e34acbae812b8a1d814c2e6285343b351c8945` |
| SourMesen/Mesen2 | `b9fa69ddc6d0a331fb103fdb5eef6904305703c2` |
| TASEmulators/fceux | `98b1e013d2e6073ed1abe1ced5ff156ad83ca744` |
| TASEmulators/BizHawk | `d4562a64b70d1948ac9c9433c5178ce3cea053b1` |
| dolphin-emu/dolphin | `3eb30c7620b4475b2e0404228ba0d2b238a132f1` |
| hrydgard/ppsspp | `43a69b1c84dfe29e82b46bd3797a1106cbd65c5b` |
| azahar-emu/azahar | `1edf59ef592a854ccc2a7405d9aade477b2a0b3d` |
| gonetz/GLideN64 | `a04081ad6a966a9413bea63a55f502e0dc98bb18` |
| panda-re/panda | `36502113107ff260e7499065c9d4b764d387a7da` |
| crates.io | aseprite-io 0.2.0, asefile 0.3.8, aseprite-loader 0.4.2, png 0.18.1, midly 0.5.3, hound 3.5.1 |

Not verified: NO$GBA's export options, the Dytan and PANDA (CC 2013)
papers' own numbers, and whether any GBA emulator ever had tile
replacement (a 2014–16 Dolphin forum thread says GBE+ did it for GB/GBC
only).
