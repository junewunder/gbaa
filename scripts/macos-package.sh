#!/usr/bin/env bash
# Puts a recompiled game's macOS build in DIR (`just macos-app`): gbaa-NAME.app,
# an Apple silicon app bundle holding the binary, with the ROM inside it,
# and README.txt beside it. docs/research/macos-build.md says why it's
# built on a Mac.
#
# The binary writes the save (NAME.sav), its backups and the save states
# to ~/Library/Application Support/gbaa/saves/NAME/ (desktop's `game_dir`),
# never into the bundle: an app bundle may be read-only (an app run from a disk image, or moved
# by Gatekeeper's translocation), and writing into it breaks its signature.
#
# ICON, if given, is a square PNG (box art, say) made into the bundle's
# icon with sips and iconutil, so on a Mac only. The menu bar and Dock show
# ROM's file name ("Kirby - Nightmare in Dream Land"), not NAME.
#
# Fails if the binary isn't an arm64 Mach-O, or (on a Mac) links a library
# outside macOS itself, e.g. one from Homebrew, which another Mac lacks.
set -eu
dir=$1 name=$2 bin=$3 rom=$4 icon=${5:-}
app="$dir/gbaa-$name.app"
title=$(basename "$rom" .gba)

case "$(file -b "$bin")" in
*Mach-O*arm64*) ;;
*) echo "macos-package: $bin is not an arm64 Mach-O binary: $(file -b "$bin")" >&2; exit 1 ;;
esac
if command -v otool > /dev/null; then
    for lib in $(otool -L "$bin" | tail -n +2 | awk '{print $1}'); do
        case "$lib" in
        /usr/lib/* | /System/Library/Frameworks/*) ;;
        *) echo "macos-package: gbaa-$name links $lib, which another Mac may lack" >&2; exit 1 ;;
        esac
    done
fi

# A fresh bundle each time; the saves live outside it.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/gbaa-$name"
chmod +x "$app/Contents/MacOS/gbaa-$name"

icon_plist=
if [ -n "$icon" ]; then
    set_dir=$(mktemp -d)/icon.iconset
    mkdir -p "$set_dir"
    for px in 16 32 128 256 512; do
        sips -s format png -z $px $px "$icon" --out "$set_dir/icon_${px}x${px}.png" > /dev/null
        sips -s format png -z $((px * 2)) $((px * 2)) "$icon" --out "$set_dir/icon_${px}x${px}@2x.png" > /dev/null
    done
    iconutil -c icns "$set_dir" -o "$app/Contents/Resources/icon.icns"
    icon_plist="<key>CFBundleIconFile</key>
	<string>icon</string>"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>$title</string>
	<key>CFBundleExecutable</key>
	<string>gbaa-$name</string>
	<key>CFBundleIdentifier</key>
	<string>dev.gbaa.$name</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>$title</string>
	$icon_plist
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>0.1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.games</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST
printf 'APPL????' > "$app/Contents/PkgInfo"

# Apple silicon runs only signed code: the linker signs the binary ad hoc,
# and this signs the bundle (its Info.plist) the same way.
# Ad hoc is enough on the Mac that built it; another Mac's Gatekeeper
# wants it allowed by hand the first time (README.txt says how).
if command -v codesign > /dev/null; then
    codesign --force --sign - "$app/Contents/MacOS/gbaa-$name"
    codesign --force --sign - "$app"
    codesign --verify --strict "$app"
fi
if command -v plutil > /dev/null; then
    plutil -lint -s "$app/Contents/Info.plist"
fi

cat > "$dir/README.txt" <<TXT
gbaa-$name.app: $name recompiled for macOS on Apple silicon (11 or newer).

Open it from Finder, or drag it to Applications first. Built on another
Mac, it is signed ad hoc only, so the first time macOS refuses to open it.
On macOS 15 and newer: try to open it, then System Settings > Privacy &
Security, scroll down, "Open Anyway". On macOS 14 and older: right-click
it > Open. (Or in Terminal: xattr -dr com.apple.quarantine on the .app.)

The save, its backups and the save states are in
~/Library/Application Support/gbaa/saves/$name/: $name.sav (mGBA's .sav: copy
one from mGBA there under that name to carry a save over), backups/ (a
copy of it each time the game closes) and $name.state1 to .state4.

Keyboard: arrows, Z = A, X = B, A = L, S = R, Enter = Start, Delete
= Select, Tab held fast-forwards, F1-F4 (with fn on a laptop) load a
state and Shift+F1-F4 save one, R held rewinds, F9 shows Lua scripts'
console, F10 opens the menu,
Cmd+Q quits. Gamepads (Xbox, PlayStation, Switch Pro and the like, wired
or Bluetooth) work too; rebind them in ~/.config/gbaa/gamepad.conf. Running
Contents/MacOS/gbaa-$name --help in Terminal lists it all.
TXT
echo "macos-app: $app ($(du -sh "$app" | cut -f1))"
