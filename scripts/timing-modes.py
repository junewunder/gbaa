#!/usr/bin/env python3
"""Kirby & the Amazing Mirror in each `--timing` mode, side by side
(docs/research/mod-view-without-timing.md, `just timing-modes`).

Runs the recompiled builds out/kirby-{exact,static,instant} along the
timing-lab's route (the same input, 4500 frames) and compares static and
instant with exact through gbaa-timing-report. Build them first:

    just recompile ROM out/kirby-MODE --readable mods/emerald/readable/all.list --timing MODE
    just build-recompiled out/kirby-MODE
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

MODES = ["exact", "static", "instant"]
FRAMES = 4500
DUMPS = "6,300,1330,1506,1990,3700,4500"
# `just timing-lab`'s input: menu presses, then walk, run, jump, float,
# puff, slide, inhale, and over the cloud wall after frame 3000.
INPUT = (
    "start@400,start@700,a@800,a@950,a@1100,a@1300,a@1500,right@1700+60,"
    "right@1800+3,right@1806+45,a@1900+3,a@1960+30,a@2050+3,a@2062+3,"
    "a@2074+3,a@2086+3,a@2098+40,b@2200+3,down@2280+30,down@2350+3,"
    "a@2353+3,b@2420+50,left@2520+20,a@2540+3,left@2543+20,right@2650+120,"
    "a@2700+30,a@2780+3,a@2792+3,right@3000+1500,a@3000-3240/12+4,b@3300+3,"
    "a@3400-3600/12+4,b@3650+3,a@3800+3,a@3900-4100/12+4,b@4150+3,"
    "a@4250+20,b@4350+40"
)


def run(rom: Path, out: Path, mode: str) -> subprocess.Popen:
    """Start build `mode` headless along the route, capped at 3 GB."""
    shutil.rmtree(out / mode, ignore_errors=True)
    (out / mode).mkdir(parents=True)
    cmd = [
        "systemd-run", "--user", "--scope", "-q",
        "-p", "MemoryMax=3G", "-p", "MemorySwapMax=0",
        f"target/recompiled/release/gbaa-kirby-{mode}", str(rom),
        "--headless", "--frames", str(FRAMES), "--input", INPUT,
        "--script", "scripts/kirby/timing.lua",
        "--frame-hashes", str(out / f"{mode}.hashes"),
        "--wav", str(out / f"{mode}.wav"),
        "--dump-at", DUMPS, "--dump-dir", str(out / mode),
    ]
    log = open(out / f"{mode}.out", "w")
    return subprocess.Popen(cmd, stdout=log, stderr=open(out / f"{mode}.err", "w"))


def main() -> int:
    rom = Path(sys.argv[1])
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    runs = {m: run(rom, out, m) for m in MODES}
    for m, p in runs.items():
        if p.wait() != 0:
            print(f"{m}: exit {p.returncode}, see {out / (m + '.err')}", file=sys.stderr)
            return 1
    # The report compares every run with `base`: exact is the real one.
    for ext in ["out", "hashes", "wav", ""]:
        base = out / ("base" + (f".{ext}" if ext else ""))
        base.unlink(missing_ok=True)
        base.symlink_to("exact" + (f".{ext}" if ext else ""))
    (out / "base.tsv").write_text("")  # no stall log: not a timing-lab build
    report = subprocess.run(
        ["target/release/gbaa-timing-report", str(out), "static", "instant"],
        capture_output=True, text=True, check=True,
    ).stdout
    # Only the comparisons: the stall-log half is empty here.
    report = report[report.index("## static"):]
    (out / "report.txt").write_text(report)
    print(report, end="")
    return 0


if __name__ == "__main__":
    os.chdir(Path(__file__).resolve().parent.parent)
    sys.exit(main())
