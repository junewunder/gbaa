#!/usr/bin/env bash
# Puts a recompiled game's Windows build in DIR (`just windows-exe`): the
# .exe (stripped at link), with the ROM inside it, which it runs when
# started with no ROM given (double-clicked), and README.txt.
# The game keeps its save and states in %APPDATA%\gbaa\saves\NAME\, not
# in DIR. Fails if the .exe isn't x86_64 PE or imports a
# DLL that isn't part of Windows (MinGW's runtime, say).
set -eu
dir=$1 name=$2 exe=$3
# What the build imports, all part of Windows 10 and up. SDL3 loads the
# rest itself when it needs them (XInput1_4.dll, Windows.Gaming.Input,
# Direct3D, the HID and audio libraries), and copes with their absence.
allowed="advapi32.dll api-ms-win-core-synch-l1-2-0.dll api-ms-win-core-winrt-error-l1-1-0.dll bcrypt.dll
bcryptprimitives.dll combase.dll gdi32.dll imm32.dll kernel32.dll mmdevapi.dll msvcrt.dll
ntdll.dll ole32.dll oleaut32.dll propsys.dll rpcrt4.dll setupapi.dll shell32.dll user32.dll
userenv.dll version.dll winmm.dll ws2_32.dll"

mkdir -p "$dir"
objdump -f "$exe" | grep -q 'file format pei-x86-64' \
    || { echo "windows-exe: $exe is not an x86_64 Windows executable" >&2; exit 1; }
for dll in $(objdump -p "$exe" | sed -n 's/^\s*DLL Name: //p' | tr 'A-Z' 'a-z' | sort -u); do
    case " $(echo $allowed) " in
    *" $dll "*) ;;
    *) echo "windows-exe: gbaa-$name.exe imports $dll, which Windows lacks" >&2; exit 1 ;;
    esac
done
cp "$exe" "$dir/gbaa-$name.exe"

# Windows' line ends, for Notepad.
sed 's/$/\r/' > "$dir/README.txt" <<TXT
gbaa-$name: $name recompiled for Windows (10 and up, x86_64).

Copy this whole folder anywhere you can write to (Documents\\Games\\$name,
say) and double-click gbaa-$name.exe: the game is inside it, so the
.exe alone is enough to send someone. Nothing to install. A console window opens with it, where any error
shows; closing either quits. Extra options: run it from a terminal
(\`gbaa-$name.exe --help\` lists them), e.g. --scale 4 or
--fullscreen.

Keyboard: arrows = D-pad, Z = A, X = B, A = L, S = R, Enter = Start,
Backspace = Select; Ctrl+Q quits. Shift+F1 to F4 save a state, F1 to
F4 load one, F5 undoes a load; R held rewinds; F9 shows Lua scripts'
console; Tab held fast-forwards,
\` toggles it. F10 opens gbaa's menu (it pauses the game; fast-forward's
speed, the gamepad's controls, quitting).

Gamepads: Xbox pads (XInput) and most others (PlayStation, Switch Pro,
8BitDo...) work once plugged in. D-pad or left stick; the face buttons
labelled A and B are the GBA's A and B, and the ones across from them
press the same (Y = A, X = B); bumpers or triggers are L and R; Start,
and Select (Back or View). Saving and loading a state are on the
keyboard (above) unless you bind a button; the right stick pushed left
rewinds; right stick click held fast-forwards, left stick click toggles
it.
Start and Select together open the menu, whose Controls screen rebinds
them. The mapping is kept in %APPDATA%\\gbaa\\gamepad.conf, and
fast-forward's speed (a line such as \`fast-speed = 1.5\`) in
%APPDATA%\\gbaa\\settings.conf.

The save and states are kept in %APPDATA%\\gbaa\\saves\\$name\\, not in
this folder, so replacing the .exe keeps them. The cartridge save there
is $name.sav, written a moment after the game saves, as mGBA's .sav:
copy one from mGBA under that name to carry a save over. Each time the
game closes it is also copied to
backups\\$name-YYYYMMDD-HHMMSS.sav (local time), unless the newest copy
there is the same: to undo a bad save, quit, copy the backup you want
over $name.sav and start again. Save states are $name.state1 to .state4.

Cross-compiled from Linux (MinGW-w64) and checked under Wine, which runs
it to the same frames as the Linux build (\`just windows-check\` in gbaa).
TXT
echo "windows-exe: $dir (gbaa-$name.exe: $(du -h "$dir/gbaa-$name.exe" | cut -f1))"
