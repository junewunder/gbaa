#!/usr/bin/env python3
"""Compare the interpreted and recompiled runs' sound (#52, `just audio-check`).

usage: audio-check.py DIR

DIR holds audio-{interp,recompiled}.{txt,fifo,log} from gbaa-desktop's
--audio-hash, --fifo-dump and --audio-log. Fails unless:
- neither run dropped a sample;
- every frame from 10 on has exactly 280896 / period samples (304 at
  18157 Hz, 176 at 10512 Hz), in both runs;
- the two audio hashes are identical. On a mismatch, prints the first
  differing record and its offset from the last buffer swap before it (a
  timer IRQ in the FIFO clock's cascade chain) and from the last FIFO DMA
  restart. Within one sample of a swap is a decision-11 timing question for
  the overseer.
"""

import re
import sys

FRAME_CYCLES = 280896
STEADY_FROM = 10


def read(d, run):
    txt = open(f"{d}/audio-{run}.txt").read()
    m = re.search(r"^audio (\d+) records hash ([0-9a-f]+)$", txt, re.M)
    dropped = re.search(r"^audio dropped (\d+)$", txt, re.M)
    if not m or not dropped:
        sys.exit(f"FAIL: no audio lines in {d}/audio-{run}.txt")
    frames, restarts, swaps = [], [], []
    with open(f"{d}/audio-{run}.log") as f:
        for line in f:
            v = line.split()
            frames.append(int(v[1]))
            restarts += [int(x[1:]) for x in v[2:] if x[0] == "r"]
            swaps += [int(x[1:]) for x in v[2:] if x[0] == "s"]
    return int(m[1]), m[2], int(dropped[1]), frames, restarts, swaps


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    d = sys.argv[1]
    period = None
    err = re.search(r"period (\d+)", open(f"{d}/audio-interp.err").read())
    if err:
        period = int(err[1])
    if not period or FRAME_CYCLES % period:
        sys.exit(f"FAIL: sample period {period} doesn't divide a frame")
    per_frame = FRAME_CYCLES // period
    ok = True
    runs = {}
    for run in ("interp", "recompiled"):
        n, h, dropped, frames, _, _ = runs[run] = read(d, run)
        print(f"{run}: {n} samples, hash {h}, dropped {dropped}")
        if dropped:
            print(f"FAIL: {run} dropped {dropped} samples")
            ok = False
        off = [(i, c) for i, c in enumerate(frames) if i >= STEADY_FROM and c != per_frame]
        if off:
            print(f"FAIL: {run}: {len(off)} frames from {STEADY_FROM} on without {per_frame} samples, first {off[0]}")
            ok = False
        else:
            print(f"{run}: {per_frame} samples every frame from {STEADY_FROM} to {len(frames) - 1}")
    (_, hi, *_), (_, hr, *_) = runs["interp"], runs["recompiled"]
    if hi == hr:
        print(f"audio hashes match: {hi}")
    else:
        ok = False
        a = open(f"{d}/audio-interp.fifo", "rb").read()
        b = open(f"{d}/audio-recompiled.fifo", "rb").read()
        i = next((i for i in range(min(len(a), len(b))) if a[i] != b[i]), min(len(a), len(b)))
        rec = i // 2
        def since(marks):
            before = [r for r in marks if r <= rec]
            return rec - before[-1] if before else None

        _, _, _, _, restarts, swaps = runs["interp"]
        print(
            f"FAIL: audio hashes differ (interp {hi}, recompiled {hr}); first differing record {rec} "
            f"(FIFO {'AB'[i % 2]}: interp {a[i:i+1].hex()} recompiled {b[i:i+1].hex()}), "
            f"{since(swaps)} samples after the last swap, {since(restarts)} after the last restart"
        )
        if swaps:
            near = min(swaps, key=lambda w: abs(w - rec))
            print(f"nearest swap: record {near} ({rec - near:+d}); within one sample is the stop condition")
    sys.exit(0 if ok else 1)


main()
