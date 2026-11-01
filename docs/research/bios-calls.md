# BIOS calls the owner's ROMs make

Survey of 2026-10-26 (bios/swi-survey). Made with
`disc-check swis ~/ROMs/gba/*.gba` (`just swis`): every `swi` among the
instructions code discovery decoded, in ROM and in the IWRAM copies the ROM
runs (`discovery::overlays`), not a byte scan. A site is *confirmed* when
descent from the entry reached it, *scanned* when only the pointer scan
found it. Unimplemented means `runtime::bios::call::IMPLEMENTED` (which a
test keeps equal to what `bios::swi` runs) doesn't list it.

## Unimplemented calls, by game

| Call | Name | Games (sites) | Unit |
|---|---|---|---|
| 0x25 | MultiBoot | Kirby: Nightmare in Dream Land (confirmed), Kirby & The Amazing Mirror, Pokemon Emerald, Emerald Imperium, Emerald Rogue, Super Mariomon | bios/multiboot-hardreset |
| 0x28 | SoundDriverVSyncOff | Kirby: Nightmare in Dream Land (confirmed) | bios/sound-driver-stubs |
| 0x19 | SoundBias | Kirby & The Amazing Mirror (2 sites) | bios/sound-bias |

No ROM makes 0x16-0x18 (the diff unfilters), 0x1A-0x1E or 0x20-0x24 (the
sound driver and music player), 0x1F (MidiKey2Freq), 0x26 (HardReset),
0x29 or 0x2A. Emerald's other "unimplemented" numbers (0x42, 0x54, 0x64,
0x73, 0xb7, ARM, scanned, around 0x087f2250) are data the scan took for
ARM code: no BIOS has those calls. Its CustomHalt (0x27) at 0x087f22d0 is
in the same run, so likely data as well.

Every game except an agb ROM, WarioWare and moonbeam carries the
SDK's table of one-call stubs (a run of `swi; bx lr` pairs, e.g. Emerald's
at 0x082e7078), so a listed call is one the game links, not necessarily one
it reaches; the confirmed ones are reached from the entry. MultiBoot's stub
in Kirby: Nightmare in Dream Land is reached, so it is the likeliest next
stop after HuffUnComp (0x13, fixed in ccb10ab).

## Every call, by game

`c` confirmed, `s` scanned only, `-` not made; **bold** = unimplemented.

| Call | Name | ambient | Imperium | Rogue | Kirby NiDL | Kirby AM | Emerald | Mariomon | WarioWare |
|---|---|---|---|---|---|---|---|---|---|
| 0x00 | SoftReset | - | s | c | c | s | s | s | - |
| 0x01 | RegisterRamReset | - | c | c | c | s | c | c | - |
| 0x03 | Stop | - | - | - | - | s | - | - | - |
| 0x05 | VBlankIntrWait | c | s | c | - | s | - | s | - |
| 0x06 | Div | - | s | c | c | s | s | s | c |
| 0x08 | Sqrt | - | s | s | - | s | s | s | - |
| 0x0A | ArcTan2 | - | s | s | s | s | s | s | - |
| 0x0B | CpuSet | c | c | c | c | c | c | c | - |
| 0x0C | CpuFastSet | - | s | c | c | s | s | s | - |
| 0x0E | BgAffineSet | - | s | s | - | - | s | s | - |
| 0x0F | ObjAffineSet | - | s | s | - | - | s | s | - |
| 0x11 | LZ77UnCompWram | - | s | s | c | s | s | s | - |
| 0x12 | LZ77UnCompVram | - | s | s | c | s | s | s | - |
| 0x13 | HuffUnComp | - | - | - | c | - | - | - | - |
| 0x14 | RLUnCompWram | - | s | - | - | s | s | s | - |
| 0x15 | RLUnCompVram | - | s | - | - | s | s | s | - |
| 0x19 | **SoundBias** | - | - | - | - | s | - | - | - |
| 0x25 | **MultiBoot** | - | s | s | c | s | s | s | - |
| 0x27 | CustomHalt | - | - | - | - | - | s | - | - |
| 0x28 | **SoundDriverVSyncOff** | - | - | - | c | - | - | - | - |

moonbeam.gba makes no `swi` discovery finds. WarioWare shows only Div:
its other calls, if any, sit in code discovery doesn't reach (EWRAM
copies aren't discovered at all), so its column is a floor, not a list.
The ARM LZ77UnCompWram sites far into the ROM (Emerald 0x089a6724, the
hacks' 0x08ed6704 and 0x091e4e00, Mariomon 0x08fb2590) are scanned and
likely data too.
