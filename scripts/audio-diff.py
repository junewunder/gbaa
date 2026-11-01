#!/usr/bin/env python3
"""Compare our sound FIFOs with mGBA's (`just mgba-audio-diff`).

usage: audio-diff.py DIR

DIR holds {mgba,ours}.fifo (`--fifo-dump`: one `A B` byte pair a record,
a record per timer overflow that pops a FIFO) and {mgba,ours}.log
(`--audio-log`: `frame records` lines; ours adds `r<i>` restarts and
`s<i>` swaps). Passes only if the two dumps are byte-identical. On a
mismatch it prints the first differing record, the frame it falls in, its
offset from our nearest buffer swap, the first frame whose record count
differs, and, if mGBA's stream is ours shifted (game code that ran at a
different speed: we have no memory wait states), the shift and how far
past the mismatch the shifted streams agree.

Then {mgba,ours}.psg (`--psg-dump`: the PSG's share of the mix, an `i16`
LE `left right` pair a sample on mGBA's grid, 512 cycles apart at the
default resolution) must be byte-identical too; on a mismatch it prints
the first differing sample, its frame, and how many samples differ.
"""

import os
import struct
import sys


def log(path):
    counts, swaps = [], []
    with open(path) as f:
        for line in f:
            v = line.split()
            counts.append(int(v[1]))
            swaps += [int(x[1:]) for x in v[2:] if x[0] == "s"]
    return counts, swaps


def frame_of(counts, rec):
    total = 0
    for f, n in enumerate(counts):
        total += n
        if rec < total:
            return f
    return len(counts)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    d = sys.argv[1]
    m = open(f"{d}/mgba.fifo", "rb").read()
    o = open(f"{d}/ours.fifo", "rb").read()
    mc, _ = log(f"{d}/mgba.log")
    oc, swaps = log(f"{d}/ours.log")
    if m == o:
        print(f"audio matches mGBA: {len(o) // 2} records")
        psg(d)
        return
    n = min(len(m), len(o))
    i = next((i for i in range(n) if m[i] != o[i]), n)
    rec = i // 2
    if i < n:
        what = f"FIFO {'AB'[i % 2]}: mgba {m[i:i+1].hex()} ours {o[i:i+1].hex()}"
    else:
        what = f"one stream ends: mgba {len(m) // 2} records, ours {len(o) // 2}"
    print(f"FAIL: audio differs from mGBA at record {rec} ({what}), frame {frame_of(oc, rec)} of ours")
    if swaps:
        near = min(swaps, key=lambda w: abs(w - rec))
        print(f"nearest swap of ours: record {near} ({rec - near:+d})")
    off = next((f for f, (a, b) in enumerate(zip(mc, oc)) if a != b), None)
    if off is not None:
        print(f"records per frame first differ in frame {off}: mgba {mc[off]}, ours {oc[off]}")
    # mGBA's record k + shift is ours' k: try shifts up to four frames' worth.
    M = [m[k:k + 2] for k in range(0, len(m), 2)]
    O = [o[k:k + 2] for k in range(0, len(o), 2)]
    best = None
    for shift in sorted(range(-1300, 1301), key=abs):
        lo = max(rec, rec - shift)
        k = lo
        while k < len(O) and k + shift < len(M) and 0 <= k + shift and O[k] == M[k + shift]:
            k += 1
        if k - lo > 1000 and (best is None or k - lo > best[2]):
            best = (shift, lo, k - lo)
    if best:
        shift, lo, run = best
        end = "to the end" if lo + run in (len(O), len(M) - shift) else f"then differ at ours {lo + run}"
        print(f"mGBA's stream is ours shifted: mgba record k+{shift} = ours k, "
              f"for {run} records from ours {lo}, {end}")
    sys.exit(1)


def psg(d):
    """Compare the PSG dumps, if both runs wrote one; exit 1 if they differ."""
    if not os.path.exists(f"{d}/mgba.psg"):
        return
    m = list(struct.iter_unpack("<hh", open(f"{d}/mgba.psg", "rb").read()))
    o = list(struct.iter_unpack("<hh", open(f"{d}/ours.psg", "rb").read()))
    if m == o:
        print(f"PSG matches mGBA: {len(o)} samples")
        return
    n = min(len(m), len(o))
    diff = [i for i in range(n) if m[i] != o[i]]
    if not diff:
        print(f"FAIL: PSG streams differ in length: mgba {len(m)} samples, ours {len(o)}")
        sys.exit(1)
    i = diff[0]
    # 280896 cycles a frame, 512 a sample at the default resolution.
    print(f"FAIL: PSG differs from mGBA at sample {i} (frame ~{i * 512 // 280896}): "
          f"mgba {m[i]} ours {o[i]}; {len(diff)} of {n} samples differ")
    for j in range(max(0, i - 3), min(n, i + 4)):
        print(f"  {j}: mgba {m[j]} ours {o[j]}{' *' if m[j] != o[j] else ''}")
    sys.exit(1)


main()
