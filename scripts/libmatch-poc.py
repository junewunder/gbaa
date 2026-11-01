#!/usr/bin/env python3
"""Proof of concept: find a reference game's functions in other ROMs by
masked byte signatures (docs/research/library-matching.md).

usage: libmatch-poc.py NAMES REF_ROM ROM...

NAMES is a source map from `just pret-names` (functions with `size` and
`file`), REF_ROM the ROM it names. Each function of 16+ fixed bytes
becomes a signature with call targets (Thumb and ARM BL/B) and literal-pool
words masked; for each ROM it prints how many of the library functions
(m4a, libgcc, the AGB SDK's libraries) and of every tenth game function
are found.

Not a tool yet. It doesn't check that a match is unique, so some hits may be
false; it knows nothing of IWRAM copies; it matches only byte-identical
code, so a different compiler (modern GCC) or library version misses; and
it's slow (about 2 minutes for 9 ROMs). The real one belongs in Rust.
"""

import tomllib, re, sys, collections, struct
if len(sys.argv) < 4:
    sys.exit(__doc__)
ROMS = sys.argv[3:]
F = tomllib.load(open(sys.argv[1], "rb"))["functions"]
emer = open(sys.argv[2], "rb").read()
def lib(f): return f.startswith(("m4a","lib","_","agb_flash")) or f in ("crt0.o","Div.o","siirtc.o")
def sig(key, size):
    a = (key & ~1) - 0x08000000
    if a < 0 or a + size > len(emer) or size < 8: return None
    b = bytearray(emer[a:a+size]); mask = [False]*size
    if key & 1:  # Thumb
        i = 0
        while i + 1 < size:
            h = struct.unpack_from("<H", b, i)[0]
            if (h & 0xF800) == 0xF000 and i + 3 < size:
                for j in range(i, i+4): mask[j] = True
                i += 4; continue
            if (h & 0xF800) == 0x4800:  # ldr rX, [pc, #imm]
                p = ((a + i + 4) & ~3) + (h & 0xFF) * 4 - a
                for j in range(p, min(p+4, size)): mask[j] = True
            i += 2
    else:        # ARM
        for i in range(0, size - 3, 4):
            w = struct.unpack_from("<I", b, i)[0]
            if (w & 0x0E000000) == 0x0A000000: mask[i] = mask[i+1] = mask[i+2] = True
            if (w & 0x0F7F0000) == 0x051F0000:
                p = a + i + 8 + ((w & 0xFFF) if w & (1 << 23) else -(w & 0xFFF)) - a
                for j in range(max(p,0), min(p+4, size)): mask[j] = True
    fixed = size - sum(mask)
    if fixed < 16: return None
    return re.compile(b"".join(b"." if m else re.escape(bytes([c])) for c, m in zip(b, mask)), re.S), fixed
groups = {"library": [], "game": []}
for k, v in F.items():
    key = int(k, 16); size = v.get("size", 0)
    s = sig(key, size)
    if s: groups["library" if lib(v.get("file","")) else "game"].append((v["name"], v.get("file",""), s))
print({g: len(x) for g, x in groups.items()}, "signatures (16+ fixed bytes)")
for rom in ROMS:
    data = open(rom, "rb").read()
    out = []
    for g in ("library", "game"):
        sigs = groups[g] if g == "library" else groups[g][::10]   # every 10th game function, for speed
        hit = sum(1 for n, f, (r, _) in sigs if r.search(data))
        out.append(f"{g} {hit}/{len(sigs)}")
        if g == "library":
            byobj = collections.Counter(f.split(".")[0].rstrip("_0123456789") for n, f, (r, _) in sigs if r.search(data))
    print(f"{rom.split('/')[-1][:40]:40} " + "  ".join(out) + "  by lib: " + ", ".join(f"{k} {c}" for k, c in byobj.most_common(6)), flush=True)
