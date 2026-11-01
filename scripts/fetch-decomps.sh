#!/usr/bin/env bash
# Clone the GBA decomps of june's other games, pinned by commit, into
# $GBAA_DATA/src and build each byte-identical to june's ROM, for its
# symbols: the Minish Cap (zeldaret/tmc), Metroid: Zero Mission
# (metroidret/mzm) and Kirby & the Amazing Mirror (jiangzhengwenjz/katam).
# Each needs its ROM as baserom (copied from ~/ROMs/gba, read only; never
# fetched) and an agbcc: pret's for tmc (from `just fetch-pret`),
# jiangzhengwenjz's fork for mzm and katam. Their names are references for
# every ROM's libraries (docs/research/cross-compiler-matching.md, "Other
# decomps"). Reruns are no-ops; to move a decomp forward, change its commit.
set -euo pipefail

data="${GBAA_DATA:-$HOME/.local/share/gbaa}"
src="$data/src"
roms="${GBAA_ROMS:-$HOME/ROMs/gba}"
tc="$data/toolchains/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi"
export PATH="$tc/bin:$PATH"
brew=/home/linuxbrew/.linuxbrew
if [[ -d "$brew/lib/pkgconfig" ]]; then
  export PKG_CONFIG_PATH="$brew/lib/pkgconfig" CPATH="$brew/include" LIBRARY_PATH="$brew/lib"
  export CMAKE_PREFIX_PATH="$brew"
fi

# checkout DIR URL COMMIT
checkout() {
  local dest=$1 url=$2 commit=$3
  if [[ -d "$dest/.git" ]] && [[ "$(git -C "$dest" rev-parse HEAD)" == "$commit" ]]; then
    return
  fi
  echo "$(basename "$dest"): fetching $url at $commit"
  mkdir -p "$dest"
  git -C "$dest" init -q
  git -C "$dest" remote remove origin 2>/dev/null || true
  git -C "$dest" remote add origin "$url"
  git -C "$dest" fetch -q --depth 1 origin "$commit"
  git -C "$dest" checkout -q --force --detach FETCH_HEAD
}

# built OUT DIR: OUT was built from DIR's checkout as it is now (OUT is
# newer than the checkout's HEAD, which moves when the commit does).
built() {
  [[ -e "$1" ]] && [[ ! "$1" -ot "$2/.git/HEAD" ]]
}

# baserom ROM SHA1 DEST: june's ROM copied in, checked.
baserom() {
  local rom=$1 sha=$2 dest=$3
  if [[ ! -f "$dest" ]]; then
    cp "$roms/$rom" "$dest"
  fi
  echo "$sha  $dest" | sha1sum -c --quiet
}

checkout "$src/tmc" https://github.com/zeldaret/tmc 6fb6dfb4a7efbe24d0fd1dda5097af6131faacde
checkout "$src/mzm" https://github.com/metroidret/mzm 7708d105cb3537f941449a9ac74795bb435493fa
checkout "$src/katam" https://github.com/jiangzhengwenjz/katam 7d969fbce14fdc838d2c1ea01389717fb96c3189
checkout "$src/agbcc-jzw" https://github.com/jiangzhengwenjz/agbcc 59b966ed1b8f371856dcf99f1546c2fe89c678ca

# The Minish Cap (USA).
tmc="$src/tmc"
if ! built "$tmc/tmc.elf" "$tmc"; then
  baserom "Legend of Zelda, The - The Minish Cap (USA).gba" b4bd50e4131b027c334547b4524e2dbbd4227130 "$tmc/baserom.gba"
  if [[ ! -x "$tmc/tools/agbcc/bin/agbcc" ]]; then
    (cd "$src/agbcc" && sh install.sh ../tmc)
  fi
  # CMake 4 refuses the json dependency's cmake_minimum_required.
  CMAKE_POLICY_VERSION_MINIMUM=3.5 make -C "$tmc" tools
  make -C "$tmc" -j8
fi

# jiangzhengwenjz's agbcc, for mzm and katam.
jzw="$src/agbcc-jzw"
if ! built "$jzw/agbcc" "$jzw"; then
  (cd "$jzw" && sh build.sh)
fi

# Metroid: Zero Mission (USA).
mzm="$src/mzm"
if ! built "$mzm/mzm_us.elf" "$mzm"; then
  baserom "Metroid - Zero Mission (USA).gba" 5de8536afe1f0078ee6fe1089f890e8c7aa0a6e8 "$mzm/mzm_us_baserom.gba"
  if [[ ! -x "$mzm/tools/agbcc/bin/agbcc" ]]; then
    (cd "$jzw" && sh install.sh ../mzm)
  fi
  make -C "$mzm" extract
  make -C "$mzm" -j8
  echo "5de8536afe1f0078ee6fe1089f890e8c7aa0a6e8  $mzm/mzm_us.gba" | sha1sum -c
fi

# Kirby & the Amazing Mirror (USA).
katam="$src/katam"
if ! built "$katam/katam.elf" "$katam"; then
  baserom "Kirby & The Amazing Mirror.gba" 274b102b6d940f46861a92b4e65f89a51815c12c "$katam/baserom.gba"
  if [[ ! -x "$katam/tools/agbcc/bin/agbcc" ]]; then
    (cd "$jzw" && sh install.sh ../katam)
  fi
  make -C "$katam" compare -j8
fi
