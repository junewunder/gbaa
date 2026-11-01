#!/usr/bin/env bash
# Clone pret's pokeemerald, pokefirered and agbcc into $GBAA_DATA/src
# (default ~/.local/share/gbaa), pinned by commit, and build pokeemerald.elf,
# pokeemerald_modern.elf and pokefirered.elf with ARM's GNU toolchain in
# $GBAA_DATA/toolchains. The builds make the ROMs from source (no ROM is
# fetched) and must match Emerald's and FireRed (U) 1.0's SHA1s. The steps
# are docs/research/discovery-prior-art.md's ("Build it instead");
# jump-tables.md has the modern build. Reruns are no-ops; to move a repo
# forward, change its commit here. FireRed is the naming loop's sibling
# control (docs/mod-view-design.md, "Measuring the leak").
set -euo pipefail

data="${GBAA_DATA:-$HOME/.local/share/gbaa}"
if [[ "$data" != /* ]]; then
  echo "GBAA_DATA must be an absolute path, got: $data" >&2
  exit 1
fi
src="$data/src"
mkdir -p "$src" "$data/toolchains"

pokeemerald_commit=731ad5bfd6e6f265508d0efcca0ba42f9dcf5881
agbcc_commit=da598c1d918402c42c0c0d7128ba14567f3175e9
emerald_sha1=f3ae088181bf583e55daf962a92bb46f4f1d07b7
pokefirered_commit=ce6406d8c2eaf46d543c8a2606de0c3cb1951982
firered_sha1=41cb23d8dccc8ebd7c649cd8fbb58eeace6e2fdc
tc_name=arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi
tc_url="https://developer.arm.com/-/media/Files/downloads/gnu/15.2.rel1/binrel/$tc_name.tar.xz"
tc="$data/toolchains/$tc_name"

# checkout DIR URL COMMIT: DIR at COMMIT (build products are left alone).
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

built() {
  local pe="$src/pokeemerald"
  [[ -d "$pe/.git" ]] && [[ "$(git -C "$pe" rev-parse HEAD)" == "$pokeemerald_commit" ]] \
    && [[ -f "$pe/pokeemerald.elf" && -f "$pe/pokeemerald_modern.elf" ]] \
    && [[ "$(sha1sum <"$pe/pokeemerald.gba" | cut -d' ' -f1)" == "$emerald_sha1" ]] \
    && [[ ! "$pe/pokeemerald.elf" -ot "$pe/.git/HEAD" ]]
}

firered_built() {
  local pf="$src/pokefirered"
  [[ -d "$pf/.git" ]] && [[ "$(git -C "$pf" rev-parse HEAD)" == "$pokefirered_commit" ]] \
    && [[ -f "$pf/pokefirered.elf" ]] \
    && [[ "$(sha1sum <"$pf/pokefirered.gba" | cut -d' ' -f1)" == "$firered_sha1" ]] \
    && [[ ! "$pf/pokefirered.elf" -ot "$pf/.git/HEAD" ]]
}

if built && firered_built; then
  echo "pokeemerald: up to date ($pokeemerald_commit)"
  echo "pokefirered: up to date ($pokefirered_commit)"
  exit 0
fi

if [[ ! -x "$tc/bin/arm-none-eabi-as" ]]; then
  echo "toolchain: downloading $tc_url"
  curl -fL --retry 3 "$tc_url" | tar -xJ -C "$data/toolchains"
fi
export PATH="$tc/bin:$PATH"
# quack has no libpng-devel: pokeemerald's tools use Homebrew's.
brew=/home/linuxbrew/.linuxbrew
if [[ -d "$brew/lib/pkgconfig" ]]; then
  export PKG_CONFIG_PATH="$brew/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export CPATH="$brew/include${CPATH:+:$CPATH}"
  export LIBRARY_PATH="$brew/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
fi

checkout "$src/agbcc" https://github.com/pret/agbcc "$agbcc_commit"
checkout "$src/pokeemerald" https://github.com/pret/pokeemerald "$pokeemerald_commit"
if [[ ! -x "$src/pokeemerald/tools/agbcc/bin/agbcc" ]]; then
  (cd "$src/agbcc" && ./build.sh && ./install.sh ../pokeemerald)
fi
if built; then
  echo "pokeemerald: up to date ($pokeemerald_commit)"
else
  make -C "$src/pokeemerald" -j8
  make -C "$src/pokeemerald" -j8 modern
  got="$(sha1sum <"$src/pokeemerald/pokeemerald.gba" | cut -d' ' -f1)"
  if [[ "$got" != "$emerald_sha1" ]]; then
    echo "pokeemerald.gba: SHA1 $got, not Emerald's $emerald_sha1" >&2
    exit 1
  fi
  echo "pokeemerald: built at $pokeemerald_commit in $src/pokeemerald"
fi

checkout "$src/pokefirered" https://github.com/pret/pokefirered "$pokefirered_commit"
if [[ ! -x "$src/pokefirered/tools/agbcc/bin/agbcc" ]]; then
  (cd "$src/agbcc" && ./install.sh ../pokefirered)
fi
make -C "$src/pokefirered" -j8 firered
got="$(sha1sum <"$src/pokefirered/pokefirered.gba" | cut -d' ' -f1)"
if [[ "$got" != "$firered_sha1" ]]; then
  echo "pokefirered.gba: SHA1 $got, not FireRed (U) 1.0's $firered_sha1" >&2
  exit 1
fi
echo "pokefirered: built at $pokefirered_commit in $src/pokefirered"
