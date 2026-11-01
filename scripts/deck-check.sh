#!/usr/bin/env bash
# `just deck-check`: runs a Steam Deck build (DIR, from `just deck-build`)
# as Steam would, through play.sh, in a clean Arch Linux container on a
# 1280x800 X screen, for SECONDS seconds. Fails unless the binary's
# libraries are all there, it runs to the end, its window covers the whole
# screen, and the picture sits in the middle 1200 columns with black bars
# at the sides. Writes DIR/../NAME-check/{screen.png,run.log}.
set -eu
dir=$(readlink -f "$1") seconds=${2:-10}
name=$(basename "$dir")
out=$(dirname "$dir")/"$name"-check
mkdir -p "$out"
image=gbaa-deck-check
podman build -q -t "$image" -f scripts/deck-check.Containerfile scripts >/dev/null
frames=$(( seconds * 60 ))
# Inside: a copy of the build (the run writes its .sav in the container),
# ldd's verdict, then play.sh under Xvfb, with the screen and the
# window tree taken halfway.
podman run --rm -v "$dir":/build:ro,Z -v "$out":/out:Z "$image" bash -c "
    set -u
    cp -r /build /game
    ldd /game/gbaa-$name > /out/ldd.txt
    Xvfb :1 -screen 0 1280x800x24 -nolisten tcp 2>/dev/null &
    export DISPLAY=:1
    for i in \$(seq 50); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.1; done
    /game/play.sh --frames $frames > /out/run.log 2>&1 &
    game=\$!
    sleep $(( seconds / 2 ))
    xwininfo -root -tree > /out/tree.txt
    xwd -root -silent > /out/screen.xwd
    wait \$game
    echo \"exit \$?\" >> /out/run.log
"
magick "$out/screen.xwd" "$out/screen.png"
rm "$out/screen.xwd"
fail() { echo "deck-check: $*" >&2; exit 1; }
cat "$out/run.log"
if grep -q 'not found' "$out/ldd.txt"; then
    grep 'not found' "$out/ldd.txt" >&2
    fail "missing libraries"
fi
tail -1 "$out/run.log" | grep -qx 'exit 0' || fail "the game failed (run.log above)"
grep -q '1280x800+0+0' "$out/tree.txt" || { cat "$out/tree.txt" >&2; fail "no window covers the 1280x800 screen"; }
[ "$(magick identify -format '%wx%h' "$out/screen.png")" = 1280x800 ] || fail "screen.png isn't 1280x800"
# The bars: the 40 columns each side all black. The picture: not all black.
bar() { magick "$out/screen.png" -crop "$1" +repage -format '%[fx:maxima]' info:; }
[ "$(bar 40x800+0+0)" = 0 ] && [ "$(bar 40x800+1240+0)" = 0 ] || fail "the side bars aren't black"
[ "$(bar 1200x800+40+0)" != 0 ] || fail "the picture is all black"
echo "deck-check: $name ran ${seconds} s fullscreen in a clean Arch container, no missing libraries ($(grep -c '=>' "$out/ldd.txt") linked), picture 1200x800 centred; $out/screen.png"
