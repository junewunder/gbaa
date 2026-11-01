#!/usr/bin/env bash
# Clone Emerald Rogue's source (Pokabbie/pokeemerald-rogue, branch expansion),
# pinned by commit, into $GBAA_DATA/src/pokeemerald-rogue and build its
# release ROM and ELF with ARM's GCC 15.2: the symbols that score xmatch's
# names for Rogue (docs/research/cross-compiler-matching.md, "Rogue with
# symbols"). The commit is EX v2.2.1, the nearest pushed source to june's
# EX v2.2.1a ROM; the build is not byte-identical to it and needn't be.
# Needs `just fetch-pret` first (its toolchain, and agbcc's headers, which
# only Rogue's dependency scan reads). Reruns are no-ops.
set -euo pipefail

data="${GBAA_DATA:-$HOME/.local/share/gbaa}"
dest="${ROGUE_DIR:-$data/src/pokeemerald-rogue}"
commit=a6adfcf18d7eaf99c2803e4b0bc04eca7af2f014
poryscript_url=https://github.com/huderlem/poryscript/releases/download/3.0.2/poryscript-linux.zip
poryscript_sha256=1080c2c48a0122f72e3325e0419ea5689ccd40bf4be3ffa2bb1cdee99abf53e6
tc="$data/toolchains/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi"
agbcc_tools="$data/src/pokeemerald/tools/agbcc"

if [[ -f "$dest/pokeemerald.elf" ]] && [[ "$(git -C "$dest" rev-parse HEAD)" == "$commit" ]]; then
  echo "pokeemerald-rogue: up to date ($commit)"
  exit 0
fi
[[ -x "$tc/bin/arm-none-eabi-gcc" && -d "$agbcc_tools/include" ]] || {
  echo "run just fetch-pret first (toolchain and agbcc headers)" >&2
  exit 1
}

if [[ ! -d "$dest/.git" ]]; then
  git clone -q --filter=blob:none --no-checkout https://github.com/Pokabbie/pokeemerald-rogue "$dest"
fi
git -C "$dest" fetch -q origin "$commit"
git -C "$dest" checkout -q --force --detach "$commit"

# The script compiler, pinned as the repo's init_deps.sh fetches it.
if [[ ! -x "$dest/tools/poryscript/poryscript-linux/poryscript" ]]; then
  mkdir -p "$dest/tools/poryscript"
  curl -fsSL -o "$dest/tools/poryscript/poryscript-linux.zip" "$poryscript_url"
  echo "$poryscript_sha256  $dest/tools/poryscript/poryscript-linux.zip" | sha256sum -c --quiet
  unzip -q -o "$dest/tools/poryscript/poryscript-linux.zip" -d "$dest/tools/poryscript"
fi
# scaninc looks for `#include "string.h"` in agbcc's headers; the modern
# build compiles with GCC's own.
ln -sfn "$agbcc_tools" "$dest/tools/agbcc"

export PATH="$tc/bin:$PATH"
brew=/home/linuxbrew/.linuxbrew
if [[ -d "$brew/lib/pkgconfig" ]]; then
  export PKG_CONFIG_PATH="$brew/lib/pkgconfig" CPATH="$brew/include" LIBRARY_PATH="$brew/lib"
fi
# Host tools: newer g++ needs <cstdint> named (mapjson's json11.cpp).
for t in tools/aif2pcm tools/bin2c tools/gbafix tools/gbagfx tools/jsonproc tools/mapjson \
  tools/mid2agb tools/preproc tools/ramscrgen tools/rsfont tools/scaninc \
  tools/Pokabbie/Build/MemoryStats tools/Pokabbie/Build/CustomJson; do
  make -s -C "$dest/$t" CXX="g++ -include cstdint"
done
make -C "$dest" -j8 RELEASE=1
echo "pokeemerald-rogue: built at $commit in $dest"
