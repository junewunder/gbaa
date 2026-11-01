#!/usr/bin/env bash
# Puts a recompiled game's Steam Deck build in DIR (`just deck-build`): the
# binary stripped, with the ROM inside it (`include_bytes!` in the
# generated crate), play.sh (what Steam runs) and README.txt (how to add
# it in Game Mode). The game keeps its save and states in
# ~/.local/share/gbaa/saves/NAME/, not in DIR. Also DIR.tar of those three
# files alone, in a NAME/ folder, for sending with
# LocalSend (which drops the executable bit on files sent loose).
# Fails if the binary needs a library SteamOS lacks or a glibc newer than
# GLIBC_MAX.
set -eu
dir=$1 name=$2 bin=$3
# What SteamOS has in its base system (libudev from systemd-libs, ALSA's
# libasound, which plays through PipeWire there). X11's and Wayland's
# libraries (libX11, libXcursor, libxkbcommon, also on SteamOS) are loaded
# with dlopen at run time, so only `just deck-check` sees them.
allowed="libudev.so.1 libasound.so.2 libgcc_s.so.1 libm.so.6 libc.so.6 ld-linux-x86-64.so.2"
# Older than any current SteamOS's glibc, which follows Arch's.
GLIBC_MAX=2.35

mkdir -p "$dir"
strip -o "$dir/gbaa-$name.tmp" "$bin"
mv "$dir/gbaa-$name.tmp" "$dir/gbaa-$name"
for lib in $(readelf -d "$dir/gbaa-$name" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
    case " $allowed " in
    *" $lib "*) ;;
    *) echo "deck-build: gbaa-$name links $lib, which SteamOS may lack" >&2; exit 1 ;;
    esac
done
glibc=$(objdump -T "$dir/gbaa-$name" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -1)
if [ "$(printf '%s\n' "$glibc" "$GLIBC_MAX" | sort -V | tail -1)" != "$GLIBC_MAX" ]; then
    echo "deck-build: gbaa-$name needs glibc $glibc, newer than $GLIBC_MAX" >&2
    exit 1
fi

cat > "$dir/play.sh" <<SH
#!/bin/sh
# What Steam runs: gbaa-$name, beside this script, fullscreen on the ROM
# it carries. Extra arguments go to gbaa-$name (\`--help\` lists them);
# a ROM path among them runs that ROM instead.
here=\$(dirname "\$(readlink -f "\$0")")
exec "\$here/gbaa-$name" --fullscreen "\$@"
SH
chmod +x "$dir/play.sh" "$dir/gbaa-$name"

cat > "$dir/README.txt" <<TXT
gbaa-$name: $name recompiled for the Steam Deck (SteamOS, x86_64).

The game is inside gbaa-$name: no ROM to copy beside it.
Copy this whole folder to the Deck, e.g. to /home/deck/Games/$name/
(in Desktop Mode, or with scp to deck@steamdeck). Nothing to install.
With LocalSend, send $name.tar instead and unpack it in /home/deck/Games/
(tar -xf $name.tar, or Dolphin's Extract Here): it makes $name/.

Adding it to Game Mode, once, in Desktop Mode:
 1. Steam > Games > Add a Non-Steam Game to My Library > Browse, pick
    play.sh in this folder (set the file type filter to All Files).
 2. Optionally rename the entry ("$name") in its Properties.
Then switch to Game Mode and launch it from the library (Non-Steam).

It runs fullscreen at 1280x800, the GBA's picture at 5x (1200x800) with
black bars at the sides, and the sound through the Deck's PipeWire.
In Desktop Mode, F11, Alt+Enter or the menu's Fullscreen row switch
between fullscreen and a window.

Controls (Steam Input's default gamepad layout): D-pad or left stick,
A or Y = GBA A, B or X = GBA B, bumpers or triggers
= L and R, Start, Select (View). The right stick pushed left rewinds
while held; right stick click held fast-forwards, left stick click
toggles it. Saving and loading a state have no button unless you bind
one (save-state and load-state in gamepad.conf, or the menu's
Controls). Start and
Select (View) together open gbaa's menu: it pauses the game and sets
fast-forward's speed; A chooses, B closes it. Quit from it, or from the
Steam button's menu (Exit game). Rebind in
~/.config/gbaa/gamepad.conf (see \`./gbaa-$name --help\`).

Fast-forward runs at 2.5x normal speed. To start at another speed, put
a line such as \`fast-speed = 1.5\` (1.1 to 16, or unlimited) in
~/.config/gbaa/settings.conf; the menu changes it until the game closes.

A stick counts as a press past 40% of its travel (the GBA's pad is all
or nothing, so a smaller dead zone presses directions you didn't mean),
a trigger past 20%. Change them in the menu's Controls, pads tab, or with
\`deadzone = 30\` and \`trigger-deadzone = 20\` (5 to 90) in settings.conf.

The save and states are kept in ~/.local/share/gbaa/saves/$name/, not in
this folder, so replacing the build keeps them. The cartridge save there
is $name.sav, written a moment after the game saves, as mGBA's .sav:
copy one from mGBA under that name to carry a save over. Each time the
game closes, it is also copied to
backups/$name-YYYYMMDD-HHMMSS.sav in that folder (the Deck's local time),
unless the newest copy there is the same: to undo a bad save or a
mistaken state load, quit, copy the backup you want over $name.sav and
start again. Save states are $name.state1 to .state4.

The build checks itself for a missing library (scripts/deck-check.sh in
gbaa: $name, launched by play.sh, in a clean Arch Linux container).
TXT
tar -C "$(dirname "$dir")" -cf "$dir.tar" \
    "$(basename "$dir")/"{"gbaa-$name",play.sh,README.txt}
echo "deck-build: $dir (gbaa-$name: $(du -h "$dir/gbaa-$name" | cut -f1), glibc $glibc)"
