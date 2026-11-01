# midi-key2freq: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/midi-key2freq.gba`. The
bitmap is drawn once, in forced blank, before the display turns on, and
never changes; check it at frame 60 (`just unit midi-key2freq` does, from
`units/midi-key2freq.toml`):

    just mgba-diff "$GBAA_DATA/agb/midi-key2freq.gba" 60

Mode 3, BG2 only. Pixel (x, y) is one `MidiKey2Freq` (swi 0x1F) call:
key `3x / 4`, fine adjust `37y mod 256`, and the WaveData frequency
`[0x2B110000, 3546000, 0xE0000000, 12345678][y % 4]`. Its result r is
drawn as `(r ^ r >> 15 ^ r >> 30) & 0x7FFF` (BGR555). mGBA's formula is
`freq / exp2f((180 - key - fine / 256) / 12)` in floats, stored to its
`int32_t` r0, so a result of 2^31 or more is `0x80000000` (colour 2): the
right end of the `0xE0000000` rows. Worked through that C (gcc, glibc):

| Pixel (x, y) | Key | Fine | r            | Colour   |
|--------------|-----|------|--------------|----------|
| (0, 0)       | 0   | 0    | `0x5622`     | `0x5622` |
| (239, 0)     | 179 | 0    | `0x28A636C0` | `0x678C` |
| (120, 1)     | 90  | 37   | `0x4D29`     | `0x4D29` |
| (239, 2)     | 179 | 74   | `0x80000000` | `0x0002` |
| (100, 3)     | 75  | 111  | `0x72DA`     | `0x72DA` |
| (0, 159)     | 0   | 251  | `0x18E`      | `0x018E` |
| (239, 159)   | 179 | 251  | `0xBC2AEE`   | `0x2B96` |
| (60, 77)     | 45  | 33   | `0x5BA`      | `0x05BA` |
