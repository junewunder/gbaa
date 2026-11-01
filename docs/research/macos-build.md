# Building for macOS: on a Mac, not cross-compiled

stretch/macos, 2026-10-24. The question: build the macOS `.app` (Apple
silicon first) by cross-compiling from the factory's Linux machines, or on
a Mac. Answer: **on a Mac** (`just macos-app ROM`, docs/tools/commands.md).
Running it needs a Mac either way, and that Mac is **june's**: the factory
has none (docs/machines.md) and no CI.

## What a macOS build needs

- **Rust** for `aarch64-apple-darwin`: a tier 1 target, `rustup target add`.
  Our own code is portable already: `desktop` uses no Linux API outside a
  Linux-only test; `libc::localtime_r` (save backups' names) is POSIX.
- **SDL3**, built from source by `sdl3`'s `build-from-source-static` with
  cmake and a C compiler. On macOS it compiles Objective-C against Apple's
  frameworks (Cocoa, Metal, QuartzCore, IOKit, GameController, CoreHaptics,
  ForceFeedback, CoreAudio, AudioToolbox, AVFoundation, Carbon...) and the
  game links them. Gamepads come through GameController and IOKit's HID.
- **cpal** plays through CoreAudio, so it links CoreAudio and AudioToolbox.
- A **linker** that writes Mach-O and **a code signature**: Apple silicon
  runs no unsigned code. Apple's `ld` signs ad hoc as it links; stripping
  afterwards voids that, so `macos-app` strips at link instead and
  `scripts/macos-package.sh` signs the bundle again with `codesign -s -`.

## Cross-compiling from Linux

Possible in principle (osxcross: clang plus a port of Apple's cctools and
ld64; or `cargo-zigbuild`, whose zig ships libSystem's stubs but no
frameworks), and `rcodesign` (the apple-codesign crate) signs from Linux.
Against it:

1. **The SDK.** Both routes need Apple's macOS SDK for the frameworks'
   headers and stubs: SDL3 and cpal can't build without it. It comes only
   inside Xcode, and the Xcode and Apple SDKs Agreement allows its use only
   on Apple-branded computers. osxcross makes you extract it yourself for
   that reason. Copying it to a Linux machine is the licence's problem,
   not a technical one, and not ours to take on.
2. **Nothing is installed for it.** quack has no clang, zig, ld64 or
   rcodesign; it would be a toolchain to build and keep working for one
   target.
3. **It saves nothing.** The `.app` must still be run on a Mac to know the
   window, sound, pads and saves work, so a Mac is in the loop whichever
   machine compiles. On that Mac the native build is supported, needs only
   Xcode's command line tools, cmake and rustup, and links exactly what
   Apple's linker expects.

## Which Mac

- **june's Mac** (the choice): `just macos-app ROM` there, then play it.
  The bundle is signed ad hoc, which is enough on the Mac that built it.
- **A CI Mac** (GitHub Actions' `macos-14`/`macos-15` runners are Apple
  silicon): would build every merge, but this repo is private, where
  macOS minutes are billed at several times Linux's, and someone would
  still have to play it. Worth it only if builds for other people's Macs
  matter; signing for them (a Developer ID and notarisation, `rcodesign`
  can do both) is the same point.

## The bundle

`gbaa-<name>.app/Contents/`: `Info.plist` (bundle id `dev.gbaa.<name>`,
macOS 11 and up, high-resolution) and `MacOS/gbaa-<name>`, the game with
its ROM built in (deck/embed-rom, 2026-10-30; before, a shell launcher ran
`MacOS/gbaa-<name>-bin` on a copy of `Resources/<name>.gba`). Run from a
bundle with no ROM given, the game takes its ROM to be
`~/Library/Application Support/gbaa/<name>/<name>.gba`
(`desktop::embedded_path`, which makes the folder), so the `.sav`, its
backups and the save states, which it writes beside its ROM, land there,
as they did with the launcher: a bundle can
be read-only (run from a disk image, or translocated by Gatekeeper) and
writing into it would void its signature. `gamepad.conf` and
`settings.conf` stay in `~/.config/gbaa/` as on Linux. Cmd+Q quits, as
Ctrl+Q does.

First built on a Mac on 2026-10-26 (an M1 Pro, macOS 15.7, Xcode's command
line tools, rustup, Homebrew's cmake): `just macos-app` on Kirby - Nightmare
in Dream Land, with its box art as `ICON`, made a 69 MB bundle whose binary
links only macOS's own frameworks and passes `codesign --verify --strict`.
Its binary ran 600 frames headless to the game-select menu in 1.4 s.
Opening the window, sound and pads on a Mac are still the play gate's.
