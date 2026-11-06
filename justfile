import 'factory/factory.just'

# The machine's build lock: every compile takes it, at 6 jobs, so lanes take turns. Recipes take it around the compile only, so running a built binary never waits; never run `just` under it (a recipe taking it under a caller that holds it deadlocks).
lock := "env CARGO_BUILD_JOBS=6 flock /tmp/gbaa-build.lock"

# A memory cap for a heavy tool: `{{cap}} MemoryMax=8G <cmd>` runs it in a systemd scope of its own with that cap and no swap. gbaa-lift, gbaa-recompile, gbaa-holes, gbaa-lift-check and route-check (but its --args and --missing) refuse to run without one (crates/analysis/src/cap.rs; GBAA_UNCAPPED=1 skips that); a cap inside one is fine, and their children run in it.
cap := "systemd-run --user --scope -q -p MemorySwapMax=0 -p"

# Lists the recipes.
default:
    @just --list

# Merge gate: format, lints, tests, and the no_std builds of `runtime` and `armv4t`.
check: check-nostd rebuildable
    cargo fmt --check
    {{lock}} cargo clippy --workspace --all-targets -- -D warnings
    {{lock}} cargo test -q --workspace --no-run
    cargo test --workspace

# Proves `runtime` and `armv4t` (decoder, semantics, interpreter) build without std (rust-toolchain.toml installs the target).
check-nostd:
    {{lock}} cargo build -p runtime -p armv4t --target riscv32imac-unknown-none-elf

# Downloads the CPU test suites into $GBAA_DATA (default ~/.local/share/gbaa). Idempotent.
fetch-tests:
    scripts/fetch-tests.sh

# SingleStepTests ARM7TDMI against the interpreter: the per-file pass table. Needs `just fetch-tests`.
sst:
    {{lock}} cargo test -q --release -p armv4t --test sst --no-run
    cargo test --release -p armv4t --test sst -- --nocapture

# SingleStepTests through the IR builder and evaluator, against the expected states and the interpreter: the same table, then blocks of opcodes run both ways. Needs `just fetch-tests`.
sst-ir:
    {{lock}} cargo test -q --release -p analysis --test sst_ir --no-run
    cargo test --release -p analysis --test sst_ir -- --nocapture --test-threads 1

# jsmolka's gba-tests in the interpreter on the runtime: pass, or the failing test's number (r12). Needs `just fetch-tests`.
gba-tests:
    #!/usr/bin/env bash
    set -u
    {{lock}} cargo build -q -p interp || exit 1
    status=0
    for t in arm thumb memory nes bios; do
        result=$(target/debug/gbaa-interp {{quote(gbaa_data / "gba-tests")}}/$t/$t.gba --test --frames 300 | tail -1) || status=1
        printf '%-8s %s\n' "$t" "$result"
    done
    exit $status

# armwrestler: walks every page of both builds with scripted input, through the interpreter and the recompiled build (RUNNER = interp, recompiled or both), dumps each to out/armwrestler-pages/, and reads its OK/BAD lines. Fails on a BAD line, a short page, or a change in the original's known failure. The recompiled runs build generated crates (heavy: run-recompiled's RAM check). Needs `just fetch-tests`.
armwrestler runner="both":
    #!/usr/bin/env bash
    set -u
    {{lock}} cargo build -q --release -p desktop -p aw-check || exit 1
    aw={{quote(gbaa_data / "armwrestler")}}
    case {{quote(runner)}} in
        both) runners="interp recompiled" ;;
        interp|recompiled) runners={{quote(runner)}} ;;
        *) echo "runner must be interp, recompiled or both" >&2; exit 2 ;;
    esac
    # Presses every 30 frames, each page dumped 20 frames after the press
    # that opens it. Start steps through the five ARM pages and back to the
    # menu; three downs reach THUMB ALU, and Start steps its three pages.
    script="start@30,start@60,start@90,start@120,start@150,start@180"
    script+=",down@210,down@225,down@240,start@270,start@300,start@330"
    pages="arm1:50 arm2:80 arm3:110 arm4:140 arm5:170 thumb1:290 thumb2:320 thumb3:350"
    # The original's seventh item, ARM V5TE, has no tests behind it: Test5
    # falls through to the Thumb launcher with test number -6, which jumps
    # through the Thumb table out of bounds. The fixed build removes it.
    v5te="down@30,down@45,down@60,down@75,down@90,down@105,start@135"
    # Every page's result count, from the source's DrawResult calls.
    results=79
    status=0
    # run RUNNER ROM FRAMES SCRIPT PNG LOG: one headless run, dumping its
    # last frame; a recompiled run's dispatcher counts (stderr) go to LOG.
    run() {
        local args=("$aw/$2" --headless --timing exact --frames "$3" --input "$4" --dump "$5")
        if [ "$1" = interp ]; then
            target/release/gbaa-desktop "${args[@]}"
        else
            just run-recompiled "out/$(basename "$2" .gba)" "${args[@]}" 2>"$6"
        fi
    }
    for runner in $runners; do
        v5te_ok=1
        for build in fixed:armwrestler-gba-fixed.gba original:armwrestler.gba; do
            name=${build%:*} rom=${build#*:}
            dir=out/armwrestler-pages/$runner/$name files=()
            mkdir -p "$dir"
            # A failed run mustn't leave the last run's pages to be read.
            rm -f "$dir"/*.png "$dir"/*.txt
            echo "$name build, $runner:"
            if [ "$runner" = recompiled ]; then
                if ! just recompile "$aw/$rom" "out/$(basename "$rom" .gba)" --timing exact > "$dir/report.txt" 2>&1; then
                    cat "$dir/report.txt"; status=1
                    [ "$name" = original ] && v5te_ok=0
                    continue
                fi
                grep -E '^(functions|discovered|indirect)' "$dir/report.txt" | sed 's/^/    /'
            fi
            for p in $pages; do
                run "$runner" "$rom" "${p#*:}" "$script" "$dir/${p%:*}.png" "$dir/recompiled.txt" || status=1
                files+=("$dir/${p%:*}.png")
            done
            if [ "$runner" = recompiled ]; then
                # The last page's run replays the whole walk. All fallback
                # would prove nothing about the generated code.
                sed 's/^/    /' "$dir/recompiled.txt"
                if grep -q '^recompiled: 0 dispatcher calls' "$dir/recompiled.txt"; then
                    echo "    no generated code ran"; status=1
                fi
            fi
            target/release/aw-check "$aw/data/font8x8.pat" "${files[@]}" --expect $results || status=1
        done
        [ $v5te_ok = 1 ] || continue
        dir=out/armwrestler-pages/$runner/original
        run "$runner" armwrestler.gba 200 "$v5te" "$dir/v5te.png" "$dir/v5te-recompiled.txt" || status=1
        # The menu showing still (a missed press) isn't the known failure:
        # the screen must be cleared and stay black.
        if target/release/aw-check --blank "$dir/v5te.png"; then
            echo "ARM V5TE                 blank      (known failure: no tests behind the item)"
        else
            echo "ARM V5TE                 drew something: the known failure changed"; status=1
        fi
    done
    exit $status

# Slow decoder checks: all 2^32 ARM words against the reference decoder, and every SingleStepTests opcode against its class.
decode-sweep:
    {{lock}} cargo test -q --release -p armv4t --no-run
    cargo test --release -p armv4t -- --ignored

# Assembly-text check: armv4t's `insn!` text against arm-none-eabi-objdump's, normalised, on jsmolka's arm.gba and thumb.gba (discovered code and every word) and every instruction of pokeemerald.elf; fails on any mismatch.
insn-text:
    #!/usr/bin/env bash
    set -eu
    od={{quote(gbaa_data / "toolchains/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi/bin/arm-none-eabi-objdump")}}
    g={{quote(gbaa_data / "gba-tests")}}
    elf={{quote(gbaa_data / "src/pokeemerald/pokeemerald.elf")}}
    {{lock}} cargo build -q --release -p insn-text-check
    c=target/release/insn-text-check
    status=0
    for run in "rom $g/arm/arm.gba" "rom $g/thumb/thumb.gba" "image $g/arm/arm.gba arm" "image $g/thumb/thumb.gba thumb" "elf $elf"; do
        set -- $run
        echo "== $run"
        "$c" "$1" "$od" "${@:2}" || status=1
    done
    exit $status

# Discovery-accuracy check (decision 5): truth from the pokeemerald ELF, then precision/recall of a discovered-starts file.
gbaa_data := env("GBAA_DATA", home_directory() / ".local/share/gbaa")
roms := env("GBAA_ROMS", home_directory() / "ROMs/gba")
disc_truth := gbaa_data / "truth/pokeemerald.funcs.tsv"

# Writes the function-start truth file of any ELF (function symbols, mode from bit 0), by default to $GBAA_DATA/truth/<name>.funcs.tsv.
disc-truth elf=(gbaa_data / "src/pokeemerald/pokeemerald.elf") out="":
    #!/usr/bin/env bash
    set -eu
    elf={{quote(join(invocation_directory(), elf))}}
    out={{quote(if out == "" { "" } else { join(invocation_directory(), out) })}}
    [ -n "$out" ] || out={{quote(gbaa_data / "truth")}}/"$(basename "$elf" .elf)".funcs.tsv
    {{lock}} cargo build -q -p disc-check
    target/debug/disc-check truth "$elf" "$out"

# Scores a discovered-starts file against a truth file (by default Emerald's); exits non-zero on a misaligned ARM start.
disc-score file show="20" truth=disc_truth:
    {{lock}} cargo build -q -p disc-check
    target/debug/disc-check score {{quote(join(invocation_directory(), truth))}} {{quote(join(invocation_directory(), file))}} --show {{show}}

# Jump-table check (decision 5c): truth tables from each pokeemerald ELF and its ROM, then the recovered tables scored against them; fails on a wrong target or a new subset. Needs `$GBAA_DATA/src/pokeemerald/pokeemerald{,_modern}.{elf,gba}`.
jt-score:
    #!/usr/bin/env bash
    set -u
    {{lock}} cargo build -q --release -p jt-check || exit 1
    status=0
    # The modern build's two subsets were checked by hand: both tables have
    # a dead entry 0 (Cmd_drawlvlupbox, AddPointsOnFainting).
    # The far-jump bars (decision 5a): the research reached 547/548 and
    # 721/732; keeping a probe's answer when it reaches no return (#25)
    # and the agb rules (#30) reach these.
    for build in "pokeemerald 676 0 548" "pokeemerald_modern 584 2 726"; do
        set -- $build
        target/release/jt-check {{quote(gbaa_data / "src/pokeemerald")}}/$1.elf --expect $2 --max-subset $3 --expect-far $4 || status=1
    done
    exit $status

# Code discovery (decision 5) on a ROM: writes its discovered-starts file (by default $GBAA_DATA/disc/<name>.starts) and prints the counts.
discover rom out="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    out={{quote(out)}}
    [ -n "$out" ] || out={{quote(gbaa_data / "disc")}}/"$(basename "$rom" .gba)".starts
    {{lock}} cargo build -q --release -p disc-check
    target/release/disc-check discover "$rom" "$out"

# Discovery on a pret build in $GBAA_DATA/src/pokeemerald (pokeemerald or pokeemerald_modern), scored: precision/recall against the ELF's truth, then the tables, far jumps and code discovery found, against the ELF. Emerald must beat the gbadisasm baseline.
disc-pret build="pokeemerald":
    #!/usr/bin/env bash
    set -eu
    src={{quote(gbaa_data / "src/pokeemerald")}}
    truth={{quote(gbaa_data / "truth")}}/{{build}}.funcs.tsv
    starts={{quote(gbaa_data / "disc")}}/{{build}}.starts
    {{lock}} cargo build -q --release -p disc-check -p jt-check
    target/release/disc-check truth "$src/{{build}}.elf" "$truth"
    target/release/disc-check discover "$src/{{build}}.gba" "$starts"
    bar=""
    [ {{build}} = pokeemerald ] && bar="--min-precision 0.9987 --min-recall 0.9349"
    status=0
    target/release/disc-check score "$truth" "$starts" --show 10 $bar || status=1
    target/release/jt-check "$src/{{build}}.elf" --discover --show 5
    exit $status

# Clones pret's pokeemerald and agbcc, pinned by commit, into $GBAA_DATA/src and builds pokeemerald.elf (never fetches a ROM; the build checks its SHA1). Idempotent.
fetch-pret:
    scripts/fetch-pret.sh

# Clones Emerald Rogue's source (Pokabbie/pokeemerald-rogue, EX v2.2.1), pinned by commit, into $GBAA_DATA/src/pokeemerald-rogue and builds its release ROM and ELF with ARM's GCC 15.2: symbols to score xmatch's names for Rogue against. Needs `just fetch-pret`. Idempotent; about 2 min.
fetch-rogue:
    scripts/fetch-rogue.sh

# Clones the decomps of june's other games (zeldaret/tmc, metroidret/mzm, jiangzhengwenjz/katam), pinned by commit, into $GBAA_DATA/src and builds each byte-identical to june's ROM (copied in as baserom, checked by SHA1): their symbols name those games and are library references for the rest. Needs `just fetch-pret`. Idempotent; about 3 min the first time.
fetch-decomps:
    scripts/fetch-decomps.sh

# Source maps from the decomps' symbols (pret-names): $GBAA_DATA/names/{tmc,mzm,katam}.toml, for june's Minish Cap, Metroid: Zero Mission and Kirby & the Amazing Mirror. Needs `just fetch-decomps`.
decomp-names:
    {{lock}} cargo build -q --release -p pret-names
    target/release/pret-names {{quote(gbaa_data / "src/tmc/tmc.elf")}} {{quote(gbaa_data / "src/tmc/tmc.gba")}} {{quote(gbaa_data / "names/tmc.toml")}} 0
    target/release/pret-names {{quote(gbaa_data / "src/mzm/mzm_us.elf")}} {{quote(gbaa_data / "src/mzm/mzm_us.gba")}} {{quote(gbaa_data / "names/mzm.toml")}} 0
    target/release/pret-names {{quote(gbaa_data / "src/katam/katam.elf")}} {{quote(gbaa_data / "src/katam/katam.gba")}} {{quote(gbaa_data / "names/katam.toml")}} 0

# Rogue's source map from that build's symbols (pret-names): writes $GBAA_DATA/names/pokeemerald-rogue.toml. Needs `just fetch-rogue`.
rogue-names show="10":
    {{lock}} cargo build -q --release -p pret-names
    target/release/pret-names {{quote(gbaa_data / "src/pokeemerald-rogue/pokeemerald.elf")}} {{quote(gbaa_data / "src/pokeemerald-rogue/pokeemerald.gba")}} {{quote(gbaa_data / "names/pokeemerald-rogue.toml")}} {{show}}

# Mod view names (decision 2): pret's symbols in a pokeemerald build (or pokefirered's, BUILD=pokefirered) matched to codegen's function entries and the data its pools point at; writes $GBAA_DATA/names/<elf name>.toml and prints the report. Needs `just fetch-pret`.
pret-names build="pokeemerald" show="10":
    {{lock}} cargo build -q --release -p pret-names
    target/release/pret-names {{quote(gbaa_data / "src" / replace(build, "_modern", ""))}}/{{build}}.elf {{quote(gbaa_data / "src" / replace(build, "_modern", ""))}}/{{build}}.gba {{quote(gbaa_data / "names")}}/{{build}}.toml {{show}}

# Emerald's songs to MIDI (docs/asset-extraction-design.md, Music): 300 frames in the interpreter logging the songs m4a starts (`--m4a-log`), then `gbaa-assets songs` finds the song table from the first and writes out/assets/emerald/songs/*.mid and manifest.toml. Needs `just fetch-pret` (its ROM and names: `just pret-names`).
assets-songs rom=(gbaa_data / "src/pokeemerald/pokeemerald.gba") out="out/assets/emerald":
    {{lock}} cargo build -q --release -p interp -p assets
    mkdir -p {{out}}
    target/release/gbaa-interp {{quote(rom)}} --frames 300 --m4a-log {{out}}/m4a.log > /dev/null
    target/release/gbaa-assets songs {{quote(rom)}} {{out}}/m4a.log {{out}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}}

# The songs against pret: each .mid through pret's mid2agb with midi.cfg's flags, assembled, against pret's built .s; writes out/assets/emerald/pret-check.txt and fails below 530 exact or 530 equal as events, of 530. Run `just assets-songs` first.
assets-check-songs out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets check-pret songs {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{out}} {{quote(gbaa_data / "src/pokeemerald")}} --min-exact 530 --min-events 530

# Emerald's tile sheets, palettes and BG tilemaps (assets/graphics-export, docs/asset-extraction-design.md, Graphics): from every route recorded under out/assets/emerald (`just assets-record NAME`), writes tiles/*.png (indexed, with the palette drawn with most; a sprite sheet a frame across or down, assets/sprites), palettes/*.pal (JASC), tilemaps/*.bin and a drawn .png, sprites/*.aseprite (each sheet OAM showed, its frames and animations, round-tripped through aseprite-io and asefile) and sprites.txt, manifest.toml's [tiles], [palettes], [tilemaps] and [sprites], and the source map layer of kind-based names for blobs pret's map doesn't name ($GBAA_DATA/names/pokeemerald-assets.toml).
assets-graphics rom=(gbaa_data / "src/pokeemerald/pokeemerald.gba") out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    rm -rf {{out}}/tiles {{out}}/palettes {{out}}/tilemaps {{out}}/sprites
    target/release/gbaa-assets graphics {{quote(rom)}} {{out}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}} --names-out {{quote(gbaa_data / "names/pokeemerald-assets.toml")}}

# The graphics against pret: each blob joined to pret's INCGFX file by symbol, checked at four levels (bytes, rebuilds, palette, image) with pret's gbagfx; writes out/assets/emerald/pret-check-graphics.txt and fails below 81 at bytes or 81 at rebuilds (the new-game route with sprite sheets joined, 2026-10-28). Run `just assets-graphics` first.
assets-check-graphics out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets check-pret graphics {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{out}} {{quote(gbaa_data / "src/pokeemerald")}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}} --min-bytes 81 --min-rebuilds 81

# The sprites against pret (assets/sprites, docs/asset-extraction-design.md, Sprites and animations): each sprite sheet's PNG and its Aseprite file's frames against pret's PNG cut into frames of the sprite's size; writes out/assets/emerald/pret-check-sprites.txt with the animations found, and fails below 3 object-event and Pokémon strips at the image level (the new-game route: Brendan walking, the moving box, Lotad; 2026-10-28). Run `just assets-graphics` first.
assets-check-sprites out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets check-pret sprites {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{out}} {{quote(gbaa_data / "src/pokeemerald")}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}} --min-strips 3

# Emerald's samples, voicegroups and soundfonts (docs/asset-extraction-design.md, Samples and voicegroups): after `just assets-songs`, each song's voicegroup read to what its songs play, the cry tables found by shape, out/assets/emerald/samples/*.wav and *.pcm, voicegroups/*.toml, soundfonts/<voicegroup>.sf2, and the manifest's [samples] and [voicegroups].
assets-samples out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets samples {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{out}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}}

# The samples against pret: each .wav through pret's wav2agb (cries as pret builds them) against pret's .bin and the ROM's bytes, each .pcm against pret's; writes out/assets/emerald/pret-check-samples.txt and fails below 544 exact, of 544. Run `just assets-samples` first.
assets-check-samples out="out/assets/emerald":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets check-pret samples {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{out}} {{quote(gbaa_data / "src/pokeemerald")}} --min-exact 544

# Discovery on every ROM in $GBAA_ROMS (default ~/ROMs/gba) (or DIR): the per-ROM counts, starts files in $GBAA_DATA/disc/.
disc-all dir=roms:
    #!/usr/bin/env bash
    set -u
    {{lock}} cargo build -q --release -p disc-check || exit 1
    status=0
    for rom in {{quote(dir)}}/*.gba; do
        echo "$(basename "$rom")"
        target/release/disc-check discover "$rom" {{quote(gbaa_data / "disc")}}/"$(basename "$rom" .gba)".starts || status=1
    done
    exit $status

# BIOS calls each ROM makes (every swi in discovered code, IWRAM copies included) and which the runtime doesn't implement; docs/research/bios-calls.md.
swis dir=roms:
    {{lock}} cargo build -q --release -p disc-check
    target/release/disc-check swis {{quote(dir)}}/*.gba

# Builds the agb ROMs from their copies in $GBAA_DATA/src (never june's checkouts; copy them there first): the first-frame ROM (gba-cart1, agb 0.25), on the nightly its rust-toolchain.toml pins, release profile. Writes $GBAA_DATA/agb/<name>.elf and the ROM it loads, <name>.gba.
agb-build:
    #!/usr/bin/env bash
    set -eu
    out={{quote(gbaa_data / "agb")}}
    mkdir -p "$out"
    {{lock}} cargo build -q --release -p disc-check
    disc="$PWD/target/release/disc-check"
    for p in "gba-cart1 agb_template"; do
        set -- $p
        (cd {{quote(gbaa_data / "src")}}/$1 && env -u CARGO_TARGET_DIR -u RUSTUP_TOOLCHAIN {{lock}} cargo build -q --release)
        cp {{quote(gbaa_data / "src")}}/$1/target/thumbv4t-none-eabi/release/$2 "$out/$1.elf"
        "$disc" rom "$out/$1.elf" "$out/$1.gba"
    done

# Builds the agb test programs in this repo's agb/ (source only: #54's iwram-rewrite, the template screen units copy) from copies synced to $GBAA_DATA/src/<name>, on the nightly each pins. Writes $GBAA_DATA/agb/<name>.elf and <name>.gba.
agb-tests:
    #!/usr/bin/env bash
    set -eu
    out={{quote(gbaa_data / "agb")}}
    mkdir -p "$out"
    {{lock}} cargo build -q --release -p disc-check
    disc="$PWD/target/release/disc-check"
    for dir in agb/*/; do
        name=$(basename "$dir")
        src={{quote(gbaa_data / "src")}}/$name
        mkdir -p "$src"
        rsync -a --delete --exclude target "$dir" "$src/"
        (cd "$src" && env -u CARGO_TARGET_DIR -u RUSTUP_TOOLCHAIN {{lock}} cargo build -q --release)
        cp "$src/target/thumbv4t-none-eabi/release/$name" "$out/$name.elf"
        "$disc" rom "$out/$name.elf" "$out/$name.gba"
        echo "$out/$name.gba"
    done

# Discovery on the agb (rustc/LLVM) ROMs, scored against their ELFs: the one from `just agb-build` and the LLVM jump-table sample. Prints precision/recall (failing below each ROM's bar), then the far jumps (failing on a false one) and code discovery found. The functions agb's crt0 copies to IWRAM/EWRAM aren't in ROM's truth: the BIOS copy lines show them.
disc-agb:
    #!/usr/bin/env bash
    set -u -o pipefail
    {{lock}} cargo build -q --release -p disc-check -p jt-check || exit 1
    agb={{quote(gbaa_data / "agb")}}
    llvm={{quote(gbaa_data / "tools/jumptables/llvm-sample")}}
    status=0
    # name, ELF, ROM, precision and recall bars. The LLVM sample's misses
    # are linker veneers its switch reaches by `b`.
    for r in "gba-cart1 $agb/gba-cart1 1.0 0.98" "llvm-sample $llvm/llvm 1.0 0.57"; do
        set -- $r
        [ -f "$2.elf" ] && [ -f "$2.gba" ] || { echo "$1: no $2.elf/.gba (just agb-build)"; status=1; continue; }
        truth={{quote(gbaa_data / "truth")}}/$1.funcs.tsv
        starts={{quote(gbaa_data / "disc")}}/$1.starts
        echo "== $1"
        rm -f "$starts"
        target/release/disc-check truth "$2.elf" "$truth" > /dev/null || { status=1; continue; }
        target/release/disc-check discover "$2.gba" "$starts" | grep -E "^functions|never return|BIOS copy" || { status=1; continue; }
        target/release/disc-check score "$truth" "$starts" --show 5 --min-precision $3 --min-recall $4 || status=1
        jt=$(target/release/jt-check "$2.elf" --discover --show 5) || status=1
        grep -E "far-jump|far jump|code:" <<< "$jt"
        # No false far jump: the rule must not swallow a real call.
        grep -q "far-jump bls reached: .*, false 0," <<< "$jt" || { echo "FAIL: false far jumps"; status=1; }
    done
    exit $status

# Codegen (#31, docs/codegen-design.md): discovers ROM's code and writes it as a Rust crate to OUT (default out/<name>), with report.txt and a copy of ROM, rom.gba, that the built binary carries inside it and runs when given no ROM. The name is the game's, the key its saves are filed under, baked into the build: the ROM's file name, lowercase with each run of other characters one dash, or `--name NAME` (in FLAGS, or in place of OUT) in the same form. FLAGS go to gbaa-recompile (its usage lists them). With `--readable` or `--mod` and no `--names`, it's named from the maps in $GBAA_DATA/names that name ROM by sha1 (a library one from `just library-names` included); a plain recompile, the play build, stays unnamed.
[positional-arguments]
recompile rom out="" *flags:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    # No OUT (or a switch in its place): gbaa-recompile picks out/<name>.
    # FLAGS are passed as given, so a quoted `--name "Two Words"` stays one.
    out=("$2")
    [ -n "$2" ] || out=()
    shift 2
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" "${out[@]}" "$@" --names-dir {{quote(gbaa_data / "names")}}

# Per-function differential runs (structured/validation; "Validation" in docs/structured-codegen-design.md): picks up to FUNCS of ROM's functions entered in its first 600 interpreted frames, records their entry states, and builds out/fn-diff-<name>/ with them compiled in every configuration (the reference, each transformation off, the readable form of the picks, named and typed from the source maps in $GBAA_DATA/names/ that name ROM, and the reference with one byte of one function changed). Runs each state under 22 mutations in every form to the function's return or first yield; fails unless every configuration matches the reference (bus trace, ticks, full state) and the fault is caught. ARGS: --exact also counts the CPSR's flags at a yield. Heavy: builds a generated crate.
fn-diff rom funcs="200" *args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    out=out/fn-diff-"$name"
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --fn-diff "$rom" "$out" --names-dir {{quote(gbaa_data / "names")}} {{funcs}}
    just build-recompiled "$out"
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
        target/recompiled/release/gbaa-fn-diff-"$name" "$rom" {{args}}

# Flag liveness's yield-path flags (structured/yield-flags; step 2 of docs/structured-codegen-design.md): recompiles ROM twice, as out/<game>-liveness and out/<game>-no-liveness (gbaa-recompile --no-flag-liveness), builds both and runs route-check --same-states with ARGS ([--every N] ROUTE…, units/*.toml on ROM), which saves each build's exact state at every checkpoint (and every Nth frame) and fails unless each pair is byte-identical. Heavy: builds two generated crates.
liveness-states rom +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{lock}} cargo build -q --release -p route-check
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-liveness --timing exact > /dev/null
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-no-liveness --timing exact --no-flag-liveness > /dev/null
    just build-recompiled out/"$name"-liveness
    just build-recompiled out/"$name"-no-liveness
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --same-states "$bin"-liveness "$bin"-no-liveness {{args}}

# The CPU suites with every function readable (mod/readable-exact): readable-states, unnamed, with every function readable (mods/emerald/readable/all.list, `*`), on jsmolka's arm, thumb, memory, nes and bios tests and both armwrestler builds, each along its route (units/jsmolka-*.toml, units/armwrestler-*.toml) with ARGS (--every N). Fails if any fails. Heavy: builds fourteen small generated crates. Needs `just fetch-tests`.
readable-suites *args:
    #!/usr/bin/env bash
    set -u
    t={{quote(gbaa_data / "gba-tests")}} aw={{quote(gbaa_data / "armwrestler")}}
    status=0
    for r in "$t"/arm/arm.gba:jsmolka-arm "$t"/thumb/thumb.gba:jsmolka-thumb "$t"/memory/memory.gba:jsmolka-memory \
        "$t"/nes/nes.gba:jsmolka-nes "$t"/bios/bios.gba:jsmolka-bios \
        "$aw"/armwrestler-gba-fixed.gba:armwrestler-fixed "$aw"/armwrestler.gba:armwrestler-original; do
        just readable-states "${r%:*}" "" mods/emerald/readable/all.list {{args}} units/"${r##*:}".toml || status=1
    done
    exit $status

# The naming loop (docs/mod-view-design.md, "The naming loop"; crates/naming): `just naming prepare ROM PRET OUT [--seed N]` draws the sample (`just naming sibling ROM PRET SIB_ROM SIB_PRET OUT` the FireRed leak control's instead), `just naming ask ROM OUT ROUND [--jobs N] [--model M] [--limit N]` asks an agent (claude -p, no tools) each function's name in ROUND (0, 1, 2, nocode, recog), `just naming score OUT` writes OUT/report.md. Arguments with spaces need quoting twice ('"…"'). See docs/tools/commands.md.
naming +args:
    {{lock}} cargo build -q --release -p naming
    target/release/naming {{args}}

# The same function across a compiler change (mod/cross-compiler-match; docs/research/cross-compiler-matching.md): `score A.gba A.toml B.gba B.toml [SHOW]` matches every function of B to one of A's or none, names hidden, and scores the pairs against both source maps; `sweep` (same arguments) tries the bars side by side; `name [--frames N] A.gba A.toml [A2.gba A2.toml…] B.gba OUT.toml` names a ROM with no symbols from the references' maps (and lists what runs in its first N frames); `matrix ROM MAP…` scores every ROM on every other; `judge OUT ROM MAP… [--limit N]` asks claude -p to name the library functions `--library` leaves unnamed (spends model usage; mod/opus-library-judge); `--library` matches libraries only. Capped at 6 GB; about 1.5 min on Emerald against its modern build.
[positional-arguments]
xmatch +args:
    {{lock}} cargo build -q --release -p xmatch
    systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 target/release/xmatch "$@"

# Library names by default (mod/default-library-names): for every ROM in ROMS that no full map in $GBAA_DATA/names names, `xmatch name --library` from all five references (Emerald, FireRed, Minish Cap, Metroid: Zero Mission, Kirby & the Amazing Mirror) into $GBAA_DATA/names/<rom>-library.toml, where the readable and mod-view prints find it by the ROM's sha1. Needs `just fetch-pret`, `just pret-names` (both builds), `just fetch-decomps` and `just decomp-names`. Capped at 6 GB.
library-names roms=roms:
    {{lock}} cargo build -q --release -p xmatch
    systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 target/release/xmatch library-names {{quote(roms)}} {{quote(gbaa_data / "names")}} \
        {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{quote(gbaa_data / "names/pokeemerald.toml")}} \
        {{quote(gbaa_data / "src/pokefirered/pokefirered.gba")}} {{quote(gbaa_data / "names/pokefirered.toml")}} \
        {{quote(gbaa_data / "src/tmc/tmc.gba")}} {{quote(gbaa_data / "names/tmc.toml")}} \
        {{quote(gbaa_data / "src/mzm/mzm_us.gba")}} {{quote(gbaa_data / "names/mzm.toml")}} \
        {{quote(gbaa_data / "src/katam/katam.gba")}} {{quote(gbaa_data / "names/katam.toml")}}

# The mod view's named print (mod/source-map; docs/mod-view-design.md, "The check that proves it"): checks every function of ROM's named print against its unnamed one token by token (gbaa-recompile --names-check), then recompiles ROM unnamed and named with the source map NAMES (out/<game>-unnamed, out/<game>-named), builds both and runs route-check --same-states with ARGS ([--every N] ROUTE…), which fails unless each pair of exact states is byte-identical. Heavy: builds two generated crates.
names-states rom names +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    names={{quote(join(invocation_directory(), names))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{lock}} cargo build -q --release -p route-check
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --names-check "$rom" --names "$names"
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-unnamed --timing exact > /dev/null
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-named --timing exact --names "$names" > /dev/null
    just build-recompiled out/"$name"-unnamed
    just build-recompiled out/"$name"-named
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --same-states "$bin"-unnamed "$bin"-named {{args}}

# A mod copy's cut (mod/mod-copy; docs/mod-view-design.md, "A mod copy"): prints FUNCS (pret names or keys) readable and named from the source map NAMES (NAMES "": the maps in $GBAA_DATA/names that name ROM by sha1) into the mod at DIR (mods/<game>/<mod>/: mod.toml, src/<Name>.rs to edit, orig/<Name>.rs as cut); a re-cut three-way merges an edited src/ file. About 20 s on Emerald, not heavy.
mod-cut rom names dir +funcs:
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile {{quote(rom)}} {{ if names == "" { "--names-dir " + quote(gbaa_data / "names") } else { "--names " + quote(names) } }} --cut {{quote(dir)}} {{funcs}}

# What to cut for a change to a struct field (mod/mod-find; docs/tools/commands.md): prints every function of ROM readable and named from the source map NAMES (NAMES "": the maps in $GBAA_DATA/names that name ROM by sha1) and lists those whose print reads or writes FIELD (e.g. optionsTextSpeed), writers and readers apart, by pret file:line. About 30 s on Emerald, not heavy.
mod-find rom names field:
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --find {{quote(rom)}} {{quote(field)}} {{ if names == "" { "--names-dir " + quote(gbaa_data / "names") } else { "--names " + quote(names) } }}

# A mod copy built in (mod/mod-copy; docs/mod-view-design.md, "A mod copy"): recompiles ROM named with NAMES (out/<game>-named) and with the mod at DIR built in (out/<game>-mod, `--mod DIR`), builds both and runs route-check --same-states with ARGS ([--every N] ROUTE…), which fails unless each pair of exact states is byte-identical and prints the modded build's resumes inside modded functions. An unedited cut must pass. Heavy: builds two generated crates.
mod-states rom names dir +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    names={{quote(join(invocation_directory(), names))}}
    dir={{quote(join(invocation_directory(), dir))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{lock}} cargo build -q --release -p route-check
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-named --timing exact --names "$names" > /dev/null
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-mod --timing exact --names "$names" --mod "$dir" | grep '^mod '
    just build-recompiled out/"$name"-named
    just build-recompiled out/"$name"-mod
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --same-states "$bin"-named "$bin"-mod {{args}}

# A mod's frames beside stock's (mod/mod-run; docs/research/agent-first-mod.md, item 3): recompiles ROM as stock (out/<game>-named: named with NAMES; NAMES "" names it from the maps in $GBAA_DATA/names that name ROM by sha1) and, named the same way, with the mod at DIR built in (out/<game>-mod), builds both, runs both along ROUTE and dumps the frames --dump-at names (default the route's checkpoints). Says which pairs are the same; for each that differs writes out/mod-run/<route>/side-N.png: stock, the mod, and where they differ in red. Heavy: builds two generated crates (about 4 min each when cold).
mod-run rom names dir +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    dir={{quote(join(invocation_directory(), dir))}}
    names=({{ if names == "" { "--names-dir " + quote(gbaa_data / "names") } else { "--names " + quote(join(invocation_directory(), names)) } }})
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{lock}} cargo build -q --release -p route-check
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-named --timing exact "${names[@]}" > /dev/null
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-mod --timing exact "${names[@]}" --mod "$dir" | grep '^mod '
    just build-recompiled out/"$name"-named
    just build-recompiled out/"$name"-mod
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --mod-run "$bin"-named "$bin"-mod {{args}}

# The readable form on a whole game (mod/readable-emerald; docs/mod-view-design.md, "The readable form", its check): recompiles ROM named with NAMES (out/<game>-named, the play view) and with the readable list LIST as well (out/<game>-readable, `--readable LIST`; a line `*` is every function); NAMES "" leaves the play view unnamed and names the readable one from the maps in $GBAA_DATA/names that name ROM by sha1, builds both and runs route-check --same-states with ARGS ([--every N] ROUTE…), which fails unless each pair of exact states is byte-identical. Heavy: builds two generated crates, the readable one bigger.
readable-states rom names list +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    names={{quote(if names == "" { "" } else { join(invocation_directory(), names) })}}
    list={{quote(join(invocation_directory(), list))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    named=(--names-dir {{quote(gbaa_data / "names")}}); [ -z "$names" ] || named=(--names "$names")
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{lock}} cargo build -q --release -p route-check
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-named --timing exact "${named[@]}" > /dev/null
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name"-readable --timing exact "${named[@]}" --readable "$list" | grep '^readable'
    just build-recompiled out/"$name"-named
    just build-recompiled out/"$name"-readable
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --same-states "$bin"-named "$bin"-readable {{args}}

# The block-timed interpreter as a full-state reference (structured/block-timed-interp; validation item 1 of docs/structured-codegen-design.md): recompiles ROM into out/<game>, builds it and runs route-check --same-states with the build and the same binary with --interp-blocks (the interpreter over the compiled blocks, ticking once at each block's exit as compiled code does) and ARGS ([--every N] ROUTE…, units/*.toml on ROM); fails unless their exact states are byte-identical at every checkpoint (and every Nth frame). Heavy: builds a generated crate.
block-timed-states rom +args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    {{lock}} cargo build -q --release -p route-check
    just recompile "$rom" out/"$name" --timing exact > /dev/null
    just build-recompiled out/"$name"
    bin=target/recompiled/release/gbaa-"$name"
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --same-states "$bin" "$bin --interp-blocks" {{args}}

# Asset overrides (assets/overrides, docs/asset-extraction-design.md, Overrides): after `just assets-graphics`, every exported file encoded back to its blob's bytes (`gbaa-assets check-overrides round-trip`, writing OUT/overrides/unedited/mod.toml with them all), and OUT/overrides/edited/, Brendan's walking sprite (Aseprite) and the truck's tileset (PNG) edited. Then per RUNNER (interp, recompiled or both) the new-game route three times, plain, with the unedited mod and with the edited one (`gbaa-desktop --assets-mod`), into OUT/overrides/runs/; fails unless the unedited run draws every frame as the plain one and the edited one differs at the naming screen and the truck (frames 3800 and 5400). The recompiled runner recompiles Emerald into RECOMPILED and builds it (heavy: build-recompiled's RAM check).
assets-check-overrides runner="both" out="out/assets/emerald" recompiled="out/pokemon-emerald-version":
    #!/usr/bin/env bash
    set -eu
    case {{quote(runner)}} in
        both) runners="interp recompiled" ;;
        interp|recompiled) runners={{quote(runner)}} ;;
        *) echo "runner must be interp, recompiled or both" >&2; exit 2 ;;
    esac
    out={{quote(out)}}
    {{lock}} cargo build -q --release -p route-check -p assets -p desktop
    mapfile -t args < <(GBAA_DATA={{quote(gbaa_data)}} target/release/route-check --args units/emerald-newgame.toml)
    rom="${args[0]}"
    target/release/gbaa-assets check-overrides round-trip "$rom" "$out"
    target/release/gbaa-assets check-overrides edit "$out" --sprite 0x084975f8 --sheet 0x0836b9e0
    runs="$out/overrides/runs"
    for runner in $runners; do
        bin=(target/release/gbaa-desktop)
        if [ "$runner" = recompiled ]; then
            just recompile "$rom" {{quote(recompiled)}} --timing exact > /dev/null
            just build-recompiled {{quote(recompiled)}}
            name=$(sed -n 's/^name = "\(.*\)"/\1/p' {{quote(recompiled / "Cargo.toml")}} | head -1)
            bin=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/recompiled/release/"$name")
        fi
        for mode in plain unedited edited; do
            rm -rf "$runs/$runner-$mode"
            mkdir -p "$runs/$runner-$mode"
            m=()
            [ "$mode" = plain ] || m=(--assets-mod "$out/overrides/$mode" --assets-manifest "$out/manifest.toml")
            "${bin[@]}" "${args[@]}" --headless --frame-hashes "$runs/$runner-$mode.hashes" --dump-dir "$runs/$runner-$mode" "${m[@]}"
        done
        echo "$runner:"
        target/release/gbaa-assets check-overrides frames "$runs/$runner-"{plain,unedited,edited}.hashes --differ 3800,5400
    done

# Where palette, VRAM and OAM bytes came from along route NAME (units/NAME.toml; assets/provenance, docs/research/assets-provenance.md): builds gbaa-desktop with the provenance feature into target/prov and runs it headless on the route; at each checkpoint writes out/provenance/NAME/frame-N.prov.txt (bytes by source and transform) and frame-N.blobs.tsv (the ROM ranges and compressed blobs that reached them), and times.txt (each frame's run time).
provenance name:
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p route-check
    {{lock}} cargo build -q --release -p desktop --features provenance --target-dir target/prov
    out=out/provenance/{{name}}
    rm -rf "$out"
    mkdir -p "$out"
    mapfile -t args < <(GBAA_DATA={{quote(gbaa_data)}} target/release/route-check --args units/{{name}}.toml)
    target/prov/release/gbaa-desktop "${args[@]}" --headless --dump-dir "$out" --frame-times "$out/times.txt"
    ls "$out"/*.prov.txt

# The load and use logs for asset export along route NAME (units/NAME.toml; assets/load-log, docs/asset-extraction-design.md, Recording): runs it headless in the provenance build (target/prov, as `just provenance`) with --assets-log, writing out/assets/GAME/NAME/loads.tsv (every BIOS decompression, CpuSet, CpuFastSet and DMA whose source traces to ROM, with its full length) and uses.tsv (each distinct BG and OBJ tile use: its blob and offset, palette bank and palette blob), and the spike's checkpoint reports (frame-N.png, .prov.txt, .blobs.tsv), then checks them with `gbaa-assets check-load-log` against what reached the screen (reached.tsv) and the last frame's tiles (shown.tsv), lengths against the source map NAMES: out/assets/GAME/NAME/check.txt. Fails if any blob, range or tile is missed.
assets-record name game="emerald" names=(gbaa_data / "names/pokeemerald.toml"):
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p route-check -p assets
    {{lock}} cargo build -q --release -p desktop --features provenance --target-dir target/prov
    mapfile -t args < <(GBAA_DATA={{quote(gbaa_data)}} target/release/route-check --args units/{{name}}.toml)
    rom="${args[0]}"
    out="out/assets/{{game}}/{{name}}"
    rm -rf "$out"
    mkdir -p "$out"
    target/prov/release/gbaa-desktop "${args[@]}" --headless --dump-dir "$out" --assets-log "$out"
    target/release/gbaa-assets check-load-log "$rom" "$out" --names {{quote(names)}}

# Play ROM in the window and extract what you see as you play (assets/play-extract, docs/asset-extraction-design.md, Extract while playing): gbaa-desktop --extract OUT in a provenance build, RUNNER interp (target/prov) or recompiled (the ROM's crate from `just recompile`, built with desktop/provenance into target/recompiled-prov: about 6 min on Emerald when the runtime changed). Each session's load, use and OAM logs go to OUT/play-YYYYMMDD-HHMMSS/; on F6 and on exit every session's and route's graphics under OUT are exported into it, keeping the names an earlier export gave. ARGS go to gbaa-desktop.
play-extract rom=(roms / "Pokemon - Emerald Version.gba") out="out/assets/emerald" runner="interp" *args:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    out={{quote(out)}}
    mkdir -p "$out"
    case {{quote(runner)}} in
        interp)
            {{lock}} cargo build -q --release -p desktop --features provenance --target-dir target/prov
            target/prov/release/gbaa-desktop "$rom" --extract "$out" {{args}}
            ;;
        recompiled)
            name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
            crate=out/"$name"
            [ -f "$crate/Cargo.toml" ] || just recompile "$rom" "$crate" > /dev/null
            avail=$(free -g | awk '/^Mem:/ {print $7}')
            if [ "$avail" -lt 7 ]; then
                echo "only ${avail} GB available, need 7 GB to build $crate; try again later" >&2
                exit 1
            fi
            {{lock}} systemd-run --user --scope -q -p MemoryMax=$(( avail - 3 ))G -p MemorySwapMax=0 cargo build -q --release --manifest-path "$crate/Cargo.toml" --target-dir target/recompiled-prov --features desktop/provenance
            bin=$(sed -n 's/^name = "\(.*\)"/\1/p' "$crate/Cargo.toml" | head -1)
            systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/recompiled-prov/release/"$bin" "$rom" --extract "$out" {{args}}
            ;;
        *) echo "runner must be interp or recompiled" >&2; exit 2 ;;
    esac

# The extract-while-playing check (assets/play-extract): a copy of Emerald's export (out/assets/emerald, after `just assets-record emerald-newgame` and `just assets-graphics`) in out/assets/play-check, then a short session headless with --extract into it (the new-game route's presses, then Start and the Bag in the truck, 6,800 frames, about 30 s), then `gbaa-assets check-extract`: fails if a tile or palette blob the session showed and loaded has no manifest entry with its files, if an earlier entry is lost or renamed, or if fewer than 5 entries are new (the Bag's: 10, 2026-10-29). Writes out/assets/play-check/play-*/extract-check.txt.
play-extract-check:
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p route-check -p assets
    {{lock}} cargo build -q --release -p desktop --features provenance --target-dir target/prov
    mapfile -t args < <(GBAA_DATA={{quote(gbaa_data)}} target/release/route-check --args units/emerald-newgame.toml)
    [ -f out/assets/emerald/manifest.toml ] || { echo "no out/assets/emerald/manifest.toml: run just assets-record emerald-newgame and just assets-graphics" >&2; exit 1; }
    out=out/assets/play-check
    rm -rf "$out"
    cp -r out/assets/emerald "$out"
    target/prov/release/gbaa-desktop "${args[0]}" --headless --timing exact --frames 6800 --input "${args[4]},start@6400,a@6460" --extract "$out"
    session=$(ls -d "$out"/play-* | tail -1)
    target/release/gbaa-assets check-extract "$out" "$session" --before out/assets/emerald/manifest.toml --min-new 5

# Kirby & the Amazing Mirror's player controller and sprites (research/kirby-controller-probe, docs/research/kirby-controller.md): from reset, menu presses to the first room, then scripts/kirby/moves.lua walks, runs, jumps, floats, puffs, crouches, slides and inhales, in the provenance build with --assets-log: OUT/moves/trace.txt (Kirby's state each frame), loads.tsv, uses.tsv, oam.tsv and frame PNGs; then `gbaa-assets graphics` (OUT/tiles, palettes, sprites), `gbaa-assets sprite-tables` (OUT/sprite-tables: whole frames of every plain-Kirby animation as Aseprite and PNG, anims.txt, every animation's hitboxes.tsv; research/kirby-sprite-tables) and the mod view's cut of the controller functions named in names/kirby-amazing-mirror.toml (OUT/controller/src). Fails unless every move's state shows in the trace. About 25 s once built.
kirby-assets rom=(roms / "Kirby & The Amazing Mirror.gba") out="out/assets/kirby":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(rom)}}
    out={{quote(out)}}
    {{lock}} cargo build -q --release -p assets -p codegen --bin gbaa-assets --bin gbaa-recompile
    {{lock}} cargo build -q --release -p desktop --features provenance --target-dir target/prov
    rm -rf "$out"
    mkdir -p "$out/moves"
    target/prov/release/gbaa-desktop "$rom" --headless --timing exact --frames 2620 \
        --input 'start@400,start@700,a@800,a@950,a@1100,a@1300,a@1500' --script scripts/kirby/moves.lua \
        --dump-at 1700,1800,1900,2000,2120,2180,2270,2330,2420,2520 --dump-dir "$out/moves" \
        --assets-log "$out/moves" > "$out/moves/trace.txt"
    # Stand, walk, run, jump rise, apex, fall, float inflate, flap, exhale, crouch, slide, inhale.
    for st in 0803ff65 080412ad 08041c51 080435f9 0804464d 08044fd5 08047815 080488e1 08049a61 08040931 08042e29 08054951; do
        grep -q " st=$st " "$out/moves/trace.txt" || { echo "no state $st in $out/moves/trace.txt" >&2; exit 1; }
    done
    target/release/gbaa-assets graphics "$rom" "$out"
    # Whole frames from the game's own sprite tables: every animation plain Kirby's AnimInfo table uses, in the pink palette.
    target/release/gbaa-assets sprite-tables "$rom" "$out" --tables 0x083B909C --anim-info 0x0834C5D4:185 \
        --palette 0x084BB12C --labels scripts/kirby/anims.toml
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" --names names/kirby-amazing-mirror.toml --cut "$out/controller" \
        KirbyTask KirbyReadInput KirbyGroundAccel KirbyGroundFriction KirbyWalk KirbyJumpRise KirbyFall KirbyFloatFlap KirbySlide SpawnSlideAttack SpawnAirPuff

# Why GBA timing can't be baked in (research/why-timing-is-runtime, docs/research/why-timing-is-runtime.md): Kirby & the Amazing Mirror from reset along a fixed input (menu presses to the first room, then walks, runs, jumps, floats, puffs, slides and inhales; after frame 3000, floats over the cloud wall into ground the first 3000 frames never saw) in gbaa-desktop's timing-lab build (target/timing-lab). First TRAIN frames with every prefetch stall's real cost logged (OUT/train.tsv); then FRAMES frames, once logged (OUT/base.tsv, the real run) and once for each wrong timing: no-prefetch (the buffer never helps), zero (no stall: the data accesses free), baked (each site's most common cost in train.tsv) and table (train.tsv's most common cost per site, buffer lead and WAITCNT, without the address). Each run writes OUT/NAME.out (scripts/kirby/timing.lua: frame count, RNG, lag, Kirby's position), NAME.hashes, NAME.wav and NAME/frame-N.png; then `gbaa-timing-report` writes OUT/report.txt (and NAME/diff-N.png). About a minute and a half once built.
timing-lab rom=(roms / "Kirby & The Amazing Mirror.gba") out="out/timing-lab/kirby" train="3000" frames="4500":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(rom)}}
    out={{quote(out)}}
    {{lock}} cargo build -q --release -p desktop --features timing-lab --target-dir target/timing-lab
    rm -rf "$out"
    mkdir -p "$out"
    input='start@400,start@700,a@800,a@950,a@1100,a@1300,a@1500,right@1700+60,right@1800+3,right@1806+45,a@1900+3,a@1960+30,a@2050+3,a@2062+3,a@2074+3,a@2086+3,a@2098+40,b@2200+3,down@2280+30,down@2350+3,a@2353+3,b@2420+50,left@2520+20,a@2540+3,left@2543+20,right@2650+120,a@2700+30,a@2780+3,a@2792+3,right@3000+1500,a@3000-3240/12+4,b@3300+3,a@3400-3600/12+4,b@3650+3,a@3800+3,a@3900-4100/12+4,b@4150+3,a@4250+20,b@4350+40'
    lab() { # NAME MODE LOG FRAMES [ARGS…]
        local name=$1 mode=$2 log=$3 frames=$4
        shift 4
        GBAA_TIMING_LAB=$mode GBAA_TIMING_LAB_LOG="$log" GBAA_TIMING_LAB_STATS="$out/$name.stats" \
            target/timing-lab/release/gbaa-desktop "$rom" --headless --timing exact --frames "$frames" --input "$input" "$@"
    }
    lab train log "$out/train.tsv" {{train}} > /dev/null
    for v in base no-prefetch zero baked table; do
        mode=$v; log="$out/train.tsv"
        [ "$v" = base ] && { mode=log; log="$out/base.tsv"; }
        mkdir -p "$out/$v"
        lab "$v" "$mode" "$log" {{frames}} --script scripts/kirby/timing.lua \
            --frame-hashes "$out/$v.hashes" --wav "$out/$v.wav" \
            --dump-at 6,300,450,720,1000,1330,1650,1760,1990,2110,2400,2600,2800,3000,3300,3700,4100,4500 --dump-dir "$out/$v" > "$out/$v.out"
    done
    target/timing-lab/release/gbaa-timing-report "$out" no-prefetch zero baked table | tee "$out/report.txt"

# The mod view without timing (mod/no-timing-flag, docs/research/mod-view-without-timing.md): runs the recompiled builds out/kirby-exact, out/kirby-static and out/kirby-instant (`--timing MODE`, every function readable; build them first, see the doc) along timing-lab's 4500-frame route and compares static and instant with exact (scripts/timing-modes.py; OUT/report.txt). About a minute once built.
timing-modes rom=(roms / "Kirby & The Amazing Mirror.gba") out="out/timing-modes/kirby":
    {{lock}} cargo build -q --release -p desktop --bin gbaa-timing-report
    python3 scripts/timing-modes.py {{quote(rom)}} {{quote(out)}}

# The fuzzing spike (assets/fuzz-coverage-spike, docs/research/fuzz-coverage.md): WORKERS processes of Emerald in the interpreter (crates/fuzz, the provenance build in target/prov) for MINUTES, each playing the new-game route, then random presses from kept states, mutated saves (the crafted alice.sav, `just emerald-saves`: party, map, flags, bag, Pokédex, vars) and RAM pokes, keeping states that reach new graphics (ROM bytes reaching palette, VRAM or OAM; ROM loads) or new ROM instructions. v2 (assets/fuzz-v2) adds sweeps (a battle against each species, normal and shiny, and each trainer; a boot of each map with the bag full) and a corpus the workers share in out/fuzz/NAME/shared/. assets/fuzz-dex-bag-sweeps adds, before fuzzing, each worker's share of the Pokédex's entry pages (footprints) and of four bag passes that scroll every item, and `table` rows (computed bytes' guessed ROM tables). assets/fuzz-move-sweep then uses each worker's share of the moves in a wild battle, one a battle, for its animation, then (assets/fuzz-move-sweep-foe-side) has the foe use each on the lead. STORY, a folder of story-point saves (`just emerald-story`; assets/fuzz-story-seeds), seeds every worker: kept for good, a share of the picks, and graphics found weigh more in picking. assets/fuzz-contest-frontier-sweeps adds, after the story seeds, each contest and each Battle Frontier challenge played from the last story save (w*/scenes.tsv). assets/fuzz-pc-box-sweep adds, after the bag passes, four PC passes: every species and Unown form in the 14 boxes, each box walked on its own wallpaper, then its menus, move items and Walda's wallpapers (rows pc-N in scenes.tsv). Writes out/fuzz/NAME/w*/ (events.tsv, kept.tsv, stats.tsv), then report.txt (route against fuzz: graphics, source map symbols, instructions, pret's functions against discovery's starts, screens) and curve.tsv.
fuzz-coverage name="spike" workers="10" minutes="45" rom=(roms / "Pokemon - Emerald Version.gba") story="":
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p fuzz --features provenance --target-dir target/prov
    out=out/fuzz/{{name}}
    rm -rf "$out"
    target/prov/release/gbaa-fuzz run {{quote(rom)}} {{quote(gbaa_data / "saves/emerald/alice.sav")}} "$out" {{ if story == "" { "" } else { quote(story) } }} --workers {{workers}} --minutes {{minutes}}
    just fuzz-report {{name}}

# Kirby & The Amazing Mirror's graphics by fuzzing (assets/kirby-fuzz, docs/research/fuzz-coverage.md, Kirby): `gbaa-fuzz`'s Kirby worker (crates/fuzz/src/kirby.rs) in WORKERS processes for MINUTES: the route to the first room and on, a door to each of the 983 rooms, each copy ability, 17 screens the rooms don't open (sub-games, intro, endings, menus, pause, the collection room) and the pause help for each ability, in each of the six languages, then random presses and RAM pokes from kept states. Then the static scan scores it, the only denominator with no decomp: `OUT/scan-curve.tsv` (blobs reached every 5 minutes) and `OUT/scan/` (the blobs it missed, greyscale). Into out/fuzz/NAME.
kirby-fuzz name="kirby" workers="12" minutes="45" rom=(roms / "Kirby & The Amazing Mirror.gba"):
    {{lock}} cargo build -q --release -p fuzz --features provenance --target-dir target/prov
    {{lock}} cargo build -q --release -p assets
    rm -rf out/fuzz/{{name}}
    target/prov/release/gbaa-fuzz run {{quote(rom)}} - out/fuzz/{{name}} --workers {{workers}} --minutes {{minutes}}
    target/release/gbaa-assets scan {{quote(rom)}} out/fuzz/{{name}} --run out/fuzz/{{name}} --curve 300

# The report of a `just fuzz-coverage` run NAME again: out/fuzz/NAME/report.txt and curve.tsv.
fuzz-report name="spike":
    {{lock}} cargo build -q --release -p fuzz --features provenance --target-dir target/prov
    target/prov/release/gbaa-fuzz report out/fuzz/{{name}} {{quote(gbaa_data / "names/pokeemerald.toml")}} {{quote(gbaa_data / "truth/pokeemerald.funcs.tsv")}} {{quote(gbaa_data / "disc/Pokemon - Emerald Version.starts")}}

# Emerald graphics coverage against pret (assets/coverage-report, docs/asset-extraction-design.md, Graphics coverage): pret's image, palette and tilemap files (every INCGFX) joined to the ROM by the source map, the ones never used in play left out with the reason (built out, unreferenced, named unused or debug, cut content, tileset palette slots never loaded, battle-animation tags no script loads), and RUN (a `just fuzz-coverage` folder, scored as its route phase and whole, or a route's manifest.toml) scored against the rest by class and kind: found with palette, greyscale, unpaired, scan only (missed by RUN, found by the static scan, assets/static-scan), missing. Writes coverage.txt and coverage.tsv beside RUN, and for a fuzz run coverage-curve.tsv (its files found every 5 minutes). Needs `just fetch-pret` and `just pret-names`.
assets-coverage run="out/fuzz/spike":
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets coverage {{quote(gbaa_data / "src/pokeemerald/pokeemerald.gba")}} {{quote(gbaa_data / "src/pokeemerald")}} --names {{quote(gbaa_data / "names/pokeemerald.toml")}} --out "$(if [ -d {{quote(run)}} ]; then echo {{quote(run)}}; else dirname {{quote(run)}}; fi)" --scan $(if [ -d {{quote(run)}} ]; then echo --curve 300; fi) {{quote(run)}}

# The static scan (assets/static-scan, docs/asset-extraction-design.md, Graphics coverage): every LZ77 blob in ROM by Nintenlord's rule, no pret needed; the ones no RUN reached (`just fuzz-coverage` folders or routes' manifest.toml) written as greyscale 4bpp sheets, OUT/scan/lz_<addr>.png and OUT/scan/scan.toml, marked found by scan with no palette or layout.
assets-scan rom=(gbaa_data / "src/pokeemerald/pokeemerald.gba") out="out/assets/emerald" *runs:
    {{lock}} cargo build -q --release -p assets
    target/release/gbaa-assets scan {{quote(rom)}} {{quote(out)}} {{ if runs == "" { "" } else { "--run " + replace(runs, " ", " --run ") } }}

# The shape of every function's CFG (docs/research/structured-control-flow.md): reducible or not, loops, irreducible regions, jump tables, Ramsey nesting. TRUTH (a disc-truth .funcs.tsv) names the functions listed.
cfg-survey rom truth="":
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p codegen --bin gbaa-cfg-survey
    target/release/gbaa-cfg-survey {{quote(join(invocation_directory(), rom))}} {{ if truth == "" { "" } else { quote(join(invocation_directory(), truth)) } }}

# Disassembly (gbaa-dis, armv4t::text: objdump's UAL text): LEN bytes (hex, default 0x40) at ROM address ADDR (hex), one instruction a line, a Thumb bl pair joined; Thumb unless --arm (an odd ADDR is Thumb); --names FILE, a names map for the ROM ($GBAA_DATA/names/<game>.toml), labels named functions and the calls to them. Paths are from where it's run. Light.
[no-cd]
[positional-arguments]
dis rom addr *args:
    @{{lock}} cargo build -q --release --manifest-path {{quote(justfile_directory() / "Cargo.toml")}} -p codegen --bin gbaa-dis
    @{{quote(justfile_directory() / "target/release/gbaa-dis")}} "$@"

# Snapshots of structured codegen (structured/emit-structured; step 8 of docs/structured-codegen-design.md): for each shape the printer handles (calls, an if, a loop, nested loops, a jump table, an irreducible region), the smallest structured function of ROM, printed as the generated crate prints it, into FILE (default mods/emerald/snapshots/emerald.rs). Rerun after a printer change and commit the diff.
snapshots rom=(roms / "Pokemon - Emerald Version.gba") file="mods/emerald/snapshots/emerald.rs":
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --snapshots {{quote(rom)}} {{quote(file)}}

# The readable form's snapshot (mod/readable-print; docs/mod-view-design.md, "The check"): the functions LIST names (by pret name or key), printed readable and named from the source map NAMES, into FILE (default mods/emerald/snapshots/emerald-readable.rs). Rerun after a printer change and commit the diff.
snapshots-readable rom=(roms / "Pokemon - Emerald Version.gba") names=(gbaa_data / "names/pokeemerald.toml") list="mods/emerald/snapshots/emerald-readable.list" file="mods/emerald/snapshots/emerald-readable.rs":
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --snapshots {{quote(rom)}} {{quote(file)}} --names {{quote(names)}} --readable {{quote(list)}}

# Builds a recompiled crate in release (heavy: refuses unless `free -g` shows NEED_GB + 3 available; GBAA_GEN_NEED_GB, default 4) into target/recompiled, capped at available - 3 GB with no swap.
build-recompiled out:
    #!/usr/bin/env bash
    set -eu
    out={{quote(out)}}
    need=$(( ${GBAA_GEN_NEED_GB:-4} + 3 ))
    avail=$(free -g | awk '/^Mem:/ {print $7}')
    if [ "$avail" -lt "$need" ]; then
        echo "only ${avail} GB available, need ${need} GB to build $out; try again later" >&2
        exit 1
    fi
    # Capped at what was available less 3 GB, no swap: a build that outgrows it is OOM-killed instead of thrashing the machine.
    {{lock}} systemd-run --user --scope -q -p MemoryMax=$(( avail - 3 ))G -p MemorySwapMax=0 cargo build -q --release --manifest-path "$out/Cargo.toml" --target-dir target/recompiled

# Builds a recompiled crate (`build-recompiled`) and runs it with gbaa-desktop's arguments, capped at 3 GB of RAM and no swap so a runaway can't take the desktop down.
[positional-arguments]
run-recompiled out *args: (build-recompiled out)
    #!/usr/bin/env bash
    set -eu
    shift
    bin=$(sed -n 's/^name = "\(.*\)"/\1/p' {{quote(out / "Cargo.toml")}} | head -1)
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
        target/recompiled/release/"$bin" "$@"

# Steam Deck build of a hack (deck/deck-build): recompiles ROM, builds it, and puts in out/deck/<name>/ what june copies to the Deck: the binary stripped (the ROM built in), play.sh (what Steam runs: the game with --fullscreen) and README.txt (adding it in Game Mode). Fails if the binary links a library SteamOS lacks or needs a newer glibc. Heavy: builds the game's crate.
deck-build rom:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    just recompile "$rom" out/"$name" | grep -v '^  uncovered'
    just build-recompiled out/"$name"
    scripts/deck-package.sh out/deck/"$name" "$name" target/recompiled/release/gbaa-"$name"

# The Deck build's check: runs out/deck/NAME (`deck-build`) through play.sh in a clean Arch Linux container (podman; SteamOS's base) on a 1280x800 Xvfb screen for SECONDS (30: at 10 Rogue is mid-fade, all black); fails on a missing library, a failed run, a window not covering the screen or a picture not centred between black bars. Writes out/deck/NAME-check/.
deck-check name seconds="30":
    scripts/deck-check.sh out/deck/{{name}} {{seconds}}

# macOS build of a game (stretch/macos), on a Mac only (docs/research/macos-build.md: why not cross-compiled): recompiles ROM, builds it for Apple silicon (macOS 11 and up) and puts gbaa-<name>.app (the ROM built in) and README.txt in out/macos/<name>/; ICON, a square PNG such as box art, becomes its icon; FLAGS go to gbaa-recompile. Needs Xcode's command line tools, cmake and rustup. Fails if the binary links a library outside macOS (one from Homebrew, say). The factory's machines have no Mac, so no build lock: run it where nothing else builds.
macos-app rom icon="" *flags:
    #!/usr/bin/env bash
    set -eu
    if [ "$(uname -s)" != Darwin ]; then
        echo "macos-app: builds on a Mac only (docs/research/macos-build.md)" >&2
        exit 1
    fi
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile "$rom" out/"$name" {{flags}} | grep -v '^  uncovered'
    rustup target add aarch64-apple-darwin
    # Stripped at link, so the linker's ad hoc signature stays valid.
    MACOSX_DEPLOYMENT_TARGET=11.0 CARGO_PROFILE_RELEASE_STRIP=symbols \
        cargo build -q --release --manifest-path out/"$name"/Cargo.toml \
        --target-dir target/recompiled --target aarch64-apple-darwin
    scripts/macos-package.sh out/macos/"$name" "$name" \
        target/recompiled/aarch64-apple-darwin/release/gbaa-"$name" "$rom" \
        {{quote(if icon == "" { "" } else { join(invocation_directory(), icon) })}}

# Windows build of a game (stretch/windows), cross-compiled on Linux: recompiles ROM, builds it for x86_64-pc-windows-gnu with MinGW-w64 in a podman container (scripts/windows.Containerfile, built on first use) and puts gbaa-<name>.exe (the ROM built in: what it runs when double-clicked) and README.txt in out/windows/<name>/. Fails if the .exe imports a DLL that isn't part of Windows. Heavy: builds the game's crate (refuses unless `free -g` shows GBAA_GEN_NEED_GB, default 4, + 3 available). docs/research/windows-build.md.
windows-exe rom:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    just recompile "$rom" out/"$name" | grep -v '^  uncovered'
    just windows-image
    need=$(( ${GBAA_GEN_NEED_GB:-4} + 3 ))
    avail=$(free -g | awk '/^Mem:/ {print $7}')
    if [ "$avail" -lt "$need" ]; then
        echo "only ${avail} GB available, need ${need} GB to build $name for Windows; try again later" >&2
        exit 1
    fi
    # Capped as build-recompiled is. Stripped at link; the CMake toolchain
    # file makes SDL3's build target Windows rather than the host.
    {{lock}} podman run --rm --security-opt label=disable \
        --memory $(( avail - 3 ))g --memory-swap $(( avail - 3 ))g \
        -v "$PWD:$PWD" -v "$HOME/.cargo/registry:/opt/cargo/registry" -w "$PWD" \
        -e CARGO_BUILD_JOBS -e RUSTUP_TOOLCHAIN=stable -e CARGO_PROFILE_RELEASE_STRIP=symbols \
        -e CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
        -e CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_gnu=/usr/share/mingw/toolchain-mingw64.cmake \
        gbaa-windows cargo build -q --release --manifest-path out/"$name"/Cargo.toml \
        --target-dir target/windows --target x86_64-pc-windows-gnu
    scripts/windows-package.sh out/windows/"$name" "$name" \
        target/windows/x86_64-pc-windows-gnu/release/gbaa-"$name".exe

# The Windows build's check (stretch/windows): runs out/windows/<game>/gbaa-<game>.exe (`windows-exe`) under Wine, headless, to the first checkpoint of units/NAME.toml (or THROUGH), and the Linux build (`build-recompiled`) beside it; fails unless every frame's hash, the checkpoint's PNG, the last registers and the .sav match, and the checkpoint draws the route's hash. Writes out/units/NAME-windows/.
windows-check name through="":
    #!/usr/bin/env bash
    set -eu
    just windows-image
    {{lock}} cargo build -q --release -p route-check
    through=(); [ -z {{quote(through)}} ] || through=(--through {{quote(through)}})
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check --windows units/{{quote(name)}}.toml "${through[@]}"

# The Windows build's container (scripts/windows.Containerfile): MinGW-w64, Rust and Wine. Cached after the first build.
windows-image:
    podman build -q -t gbaa-windows -f scripts/windows.Containerfile scripts > /dev/null

# Recompiles ROM, builds it, and runs it headless for FRAMES frames both recompiled and with --interp, at TIMING (exact, mGBA's, or native, docs/native-port-design.md section 7); fails unless the last frame and the registers match. Writes out/<name>/{recompiled,interp}.{txt,png}, and the recompiled run's stderr (the dispatcher's stats) to recompiled.log.
recompile-check rom frames="60" timing="exact":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    out=out/"$name"
    just recompile "$rom" "$out" --timing {{timing}} | grep -v '^  uncovered'
    args=("$rom" --headless --timing {{timing}} --frames {{frames}} --hash --regs)
    just build-recompiled "$out"
    bin=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/recompiled/release/gbaa-"$name")
    t0=$(date +%s%N)
    "${bin[@]}" "${args[@]}" --dump "$out/recompiled.png" > "$out/recompiled.txt" 2> "$out/recompiled.log" || { cat "$out/recompiled.log"; exit 1; }
    t1=$(date +%s%N)
    "${bin[@]}" "${args[@]}" --interp --dump "$out/interp.png" > "$out/interp.txt" || { cat "$out/interp.txt"; exit 1; }
    t2=$(date +%s%N)
    cat "$out/recompiled.txt" "$out/recompiled.log"
    echo "host time: recompiled $(( (t1 - t0) / 1000000 )) ms, interpreter $(( (t2 - t1) / 1000000 )) ms"
    if diff "$out/interp.txt" "$out/recompiled.txt"; then
        echo "recompiled matches the interpreter"
    else
        echo "recompiled differs from the interpreter (above: < interp, > recompiled)"
        exit 1
    fi

# The native spike's check (native/agb-spike; docs/research/native-agb-spike.md): agb/native-spike (`just agb-tests` builds it) recompiled at --timing exact and built, crates/native-spike's program built plain and with its one planted wrong store, then `native-spike-check` runs all three on crates/native-spike/check.toml's script; fails unless play and native write the same frame hashes on every frame and the planted build's differ. Writes out/native-spike/check/. Heavy: builds a generated crate.
native-spike:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(gbaa_data / "agb/native-spike.gba")}}
    [ -f "$rom" ] || { echo "no $rom: run just agb-tests" >&2; exit 1; }
    mkdir -p out
    just recompile "$rom" out/native-spike --timing exact > out/native-spike.recompile.txt
    just build-recompiled out/native-spike
    {{lock}} cargo build -q --release -p native-spike --features plant --bin native-spike
    cp target/release/native-spike target/release/native-spike-planted
    {{lock}} cargo build -q --release -p native-spike
    target/release/native-spike-check crates/native-spike/check.toml out/native-spike/check \
        target/recompiled/release/gbaa-native-spike "$rom" target/release/native-spike target/release/native-spike-planted

# The native check (native/oracle, native/hw-sound; docs/native-port-design.md section 7, docs/tools/commands.md): agb/native-spike, agb/psg-square and agb/direct-sound (`just agb-tests`) recompiled at --timing native and built, then route-check --native runs each with its native program (crates/native-spike, check layout) on units/native-spike.toml, units/psg-square.toml and units/direct-sound.toml and compares every frame's picture, RAM (outside the stack, code copies and crates/native-spike/check/*.replaced.toml), I/O (sound and timers too), display memories and audio samples (psg-square's FIFO A and PSG, direct-sound's two FIFOs and timer-IRQ buffer swaps, mixed by the play build's own sound path in runtime::hw); for the spike also its play layout's frame hashes and a planted wrong store, and for psg-square a planted wrong sample, each of which must be caught. Writes out/native-check/. Heavy: builds three generated crates.
native-check:
    #!/usr/bin/env bash
    set -eu
    spike={{quote(gbaa_data / "agb/native-spike.gba")}}
    psg={{quote(gbaa_data / "agb/psg-square.gba")}}
    direct={{quote(gbaa_data / "agb/direct-sound.gba")}}
    for r in "$spike" "$psg" "$direct"; do
        [ -f "$r" ] || { echo "no $r: run just agb-tests" >&2; exit 1; }
    done
    mkdir -p out
    just recompile "$spike" out/native-spike --timing native > out/native-spike.recompile.txt
    just build-recompiled out/native-spike
    just recompile "$psg" out/psg-square --timing native > out/psg-square.recompile.txt
    just build-recompiled out/psg-square
    just recompile "$direct" out/direct-sound --timing native > out/direct-sound.recompile.txt
    just build-recompiled out/direct-sound
    {{lock}} cargo build -q --release -p route-check
    {{lock}} cargo build -q --release -p native-spike --features guest-layout,plant --bin native-spike --bin native-psg-square
    cp target/release/native-spike target/release/native-spike-planted-guest
    cp target/release/native-psg-square target/release/native-psg-square-planted-guest
    {{lock}} cargo build -q --release -p native-spike --features guest-layout --bin native-spike --bin native-psg-square --bin native-direct-sound
    cp target/release/native-spike target/release/native-spike-guest
    {{lock}} cargo build -q --release -p native-spike --bin native-spike
    {{cap}} MemoryMax=8G target/release/route-check --native target/release/native-spike-guest target/recompiled/release/gbaa-native-spike \
        units/native-spike.toml --replaced crates/native-spike/check/native-spike.replaced.toml \
        --packed target/release/native-spike --planted target/release/native-spike-planted-guest
    {{cap}} MemoryMax=8G target/release/route-check --native target/release/native-psg-square target/recompiled/release/gbaa-psg-square \
        units/psg-square.toml --replaced crates/native-spike/check/psg-square.replaced.toml \
        --planted target/release/native-psg-square-planted-guest
    {{cap}} MemoryMax=8G target/release/route-check --native target/release/native-direct-sound target/recompiled/release/gbaa-direct-sound \
        units/direct-sound.toml --replaced crates/native-spike/check/direct-sound.replaced.toml

# The stage-A lifter (native/lift-stage-a; docs/native-port-design.md, "The stage-A lifter as built"; docs/tools/commands.md): gbaa-lift writes ROM whole as plain Rust over byte RAM and runtime::hw to OUT (default out/lift-<name>), with the stand-ins of holes/<name>.toml if there is one, else of mods/<name>/holes.toml (a commercial game's, game-derived and private) (<name> is --name NAME's if given, else the ROM's file name lowercased), then builds it into target/recompiled. FLAGS go to gbaa-lift (--name, --holes, --bars, --drop-standin, --plant). Fails, naming each, on a hard hole no stand-in covers. Heavy: builds a generated crate.
[positional-arguments]
lift rom out="" *flags:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr '[:upper:]' '[:lower:]')
    out=${2-}
    shift 2 || shift $#
    # --name NAME (for a file name with spaces) names the crate and the holes file.
    prev=""
    for a in "$@"; do [ "$prev" = --name ] && name=$a; prev=$a; done
    out=${out:-out/lift-$name}
    holes=()
    # A game's holes file is game-derived, so it lives in the private mods/<name>/.
    if [ -f "holes/$name.toml" ]; then holes=(--holes "holes/$name.toml")
    elif [ -f "mods/$name/holes.toml" ]; then holes=(--holes "mods/$name/holes.toml"); fi
    {{lock}} cargo build -q --release -p codegen --bin gbaa-lift
    # A hard hole fails the lift, and the build too (a compile_error! names each).
    lifted=0
    {{cap}} MemoryMax=16G target/release/gbaa-lift "$rom" "$out" "${holes[@]}" "$@" || lifted=$?
    just build-recompiled "$out"
    exit $lifted

# The stage-A lifter's check (native/lift-stage-a, native/lift-bars; docs/native-stage-b.md, "How every piece is checked"): gbaa-lift-check runs the programs of crates/codegen/lift-check.toml (one table each: ROM under $GBAA_DATA, route, flags, bars; today agb/native-spike, agb/affine, agb/mosaic-modes, agb/native-fallthrough, agb/native-irq-hold, agb/native-frame-locals and agb/native-ime-release from `just agb-tests`). First each is lifted with `--bars`, failing on any metric over its bar, named with both numbers; then each lift and its play build (--timing native) are built and route-check --native compares them for the route's 600 frames (601 for native-irq-hold and native-ime-release) with no replaced-library list: pictures, RAM outside the stack and code copies, I/O, display memories. Controls from the file: the spike lifted with a planted wrong store (planted_store, the V-blank handler's frame count) must be caught, and with its IRQ wrapper's stand-in dropped (planted_hole) the lift and the crate's build must fail naming f_03000000's mrs; native-fallthrough, native-irq-hold, native-frame-locals and native-ime-release lifted with each of their planted_flags (--fall-as-tail-call: the old fall into another function as a tail call; --irq-at-waits: the old IRQ at a wait inside a critical section; --plant-slot=0x08000182: a store to a stack local, fl_keep's x + 7, made one more; --ime-at-waits: the old IRQ held past the store that sets IME again) must be caught by the native check. Each program is also lifted at the play layout (--layout packed: globals in the lifter's order; native/lift-packed-layout), which route-check --packed holds to the guest layout's frames and records. ARGS: --only NAME. Writes out/lift-*. Heavy: builds twenty-six generated crates (about 10 minutes).
lift-check *args:
    {{lock}} cargo build -q --release -p route-check
    {{lock}} cargo build -q --release -p codegen --bin gbaa-lift --bin gbaa-lift-check
    {{cap}} MemoryMax=8G target/release/gbaa-lift-check crates/codegen/lift-check.toml --data {{quote(gbaa_data)}} {{args}}

# Milestone 2's pass check (#32): `recompile-check` (at TIMING, exact unless told), then the two frame dumps (out/<name>/{interp,recompiled}.png) must be byte-identical. Prints the guest-cycle split between recompiled code and the interpreter fallback (decision 1). Heavy: builds a generated crate.
first-frame rom frames="60" timing="exact":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    just recompile-check "$rom" {{frames}} {{timing}}
    if cmp -s out/"$name"/interp.png out/"$name"/recompiled.png; then
        echo "first frame: identical ($(grep -o 'frame .* hash .*' out/"$name"/recompiled.txt)), out/$name/{interp,recompiled}.png"
    else
        echo "first frame: the PNGs differ (out/$name/{interp,recompiled}.png)"
        exit 1
    fi

# DMA sound (#52, docs/sound-design.md): recompiles ROM, runs it headless for FRAMES frames recompiled and with --interp, and fails unless the FIFO dumps' hashes match, neither run dropped a sample, and every frame from 10 on has 280896/period samples (304 at 18157 Hz, 176 at 10512). Writes out/<name>/audio-{interp,recompiled}.{txt,err,fifo,log}. Heavy: builds a generated crate.
audio-check rom frames="600":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    out=out/"$name"
    just recompile "$rom" "$out" --timing exact | grep -v '^  uncovered'
    just build-recompiled "$out"
    bin=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/recompiled/release/gbaa-"$name")
    for run in recompiled interp; do
        flag=(); [ "$run" = interp ] && flag=(--interp)
        "${bin[@]}" "$rom" --headless --timing exact --frames {{frames}} "${flag[@]}" --audio-hash \
            --fifo-dump "$out/audio-$run.fifo" --audio-log "$out/audio-$run.log" \
            > "$out/audio-$run.txt" 2> "$out/audio-$run.err" || { cat "$out/audio-$run.err"; exit 1; }
        grep '^audio rate' "$out/audio-$run.err" | sed "s/^/$run: /"
    done
    python3 -I scripts/audio-check.py "$out"

# The pacing harness (#53) on a real ROM's stream: `gbaa-desktop --audio-pace` plays FRAMES frames' worth of the interpreter's sound to a simulated 48 kHz device whose clock is 0.5% slow, exact, then 0.5% fast, each frame costing its real host time. Fails on an underrun after the first second or a queue outside 20-100 ms.
audio-pace rom frames="1200":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    {{lock}} cargo build -q --release -p desktop
    for drift in -0.5 0 0.5; do
        systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
            target/release/gbaa-desktop "$rom" --headless --timing exact --frames {{frames}} --audio-pace "$drift"
    done

# The window check (#53): ROM in a window for FRAMES frames with sound on, inside a private headless KWin session (`cu session`) whose sound goes to its `cu-silent` null sink, never the speakers. Fails unless it exits 0 and prints `audio: N samples delivered` with N above 0, so a run that found no device and went muted fails. Needs `cu`.
audio-window rom frames="600":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    {{lock}} cargo build -q --release -p desktop
    s=gbaa-audio-$$
    cu session new "$s" --fresh-profile > /dev/null
    trap 'cu session kill "$s" > /dev/null' EXIT
    eval "$(cu session env "$s")"
    # Without these the sound would reach june's speakers.
    [ "${PIPEWIRE_NODE:-}" = cu-silent ] && [ "${PULSE_SINK:-}" = cu-silent ] \
        || { echo "the session's sound isn't routed to cu-silent; not running" >&2; exit 1; }
    log=$(mktemp)
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
        target/release/gbaa-desktop "$rom" --frames {{frames}} > "$log" &
    pid=$!
    # And check where the stream really went: stop it unless every
    # gbaa-desktop stream is on cu-silent (the ALSA plugin's streams don't
    # say which process they're from, so another gbaa-desktop elsewhere
    # fails this too).
    silent=$(pactl list short sinks | awk '$2 == "cu-silent" { print $1 }')
    sink=
    for _ in $(seq 50); do
        sink=$(pactl list sink-inputs | awk '/^\tSink:/ { s = $2 } /node.name = "alsa_playback.gbaa-desktop"/ { print s }' | sort -u | paste -sd,)
        [ -n "$sink" ] || ! kill -0 "$pid" 2> /dev/null && break
        sleep 0.1
    done
    if [ -z "$silent" ] || [ "$sink" != "$silent" ]; then
        kill "$pid" 2> /dev/null || true
        echo "the sound went to sink '$sink', not cu-silent ($silent); stopped" >&2
        cat "$log"
        exit 1
    fi
    wait "$pid" || { cat "$log"; exit 1; }
    out=$(cat "$log")
    echo "$out (sink $sink, cu-silent)"
    n=$(sed -n 's/^audio: \([0-9]*\) samples delivered.*/\1/p' <<< "$out")
    [ "${n:-0}" -gt 0 ] || { echo "no sound delivered" >&2; exit 1; }

# SingleStepTests through generated Rust: PER_FILE cases from each file, one generated fn each, against the expected state and the interpreter; then SEQS random blocks (2 to 6 opcodes) and short functions with branches, against the interpreter: registers, bus accesses, timing calls and each tick's cycles (armv4t::cycles). ARGS: --readable prints each in the readable form (mod/readable-print). Needs `just fetch-tests`; builds a generated crate (heavy, same RAM check).
sst-gen per_file="500" seqs="4000" *args:
    {{lock}} cargo build -q --release -p codegen --bin gbaa-recompile
    {{cap}} MemoryMax=8G target/release/gbaa-recompile --sst out/sst-gen {{per_file}} {{seqs}} {{args}}
    just run-recompiled out/sst-gen

# Milestone 3's oracle (#55): runs ROM for FRAMES frames in mGBA (`mgba-dump`) and in our interpreter, with optional --input presses, and fails unless the last frames' hashes match. On a mismatch writes mgba.png, ours.png and diff.png (differing pixels in red) to out/mgba-diff/<name>/ and prints the count. A ROM in crates/mgba-dump/known-mismatches.tsv passes on a mismatch while both frames keep the hashes listed there, and fails once it matches until its line is removed. Builds libmgba once (scripts/fetch-mgba.sh).
mgba-diff rom frames input="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    [ -n "$name" ] || { echo "no name for $rom" >&2; exit 2; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump -p desktop
    out=out/mgba-diff/"$name"
    rm -rf "$out"
    mkdir -p "$out"
    args=("$rom" --frames {{frames}} --hash)
    [ -z {{quote(input)}} ] || args+=(--input {{quote(input)}})
    theirs=$(target/release/mgba-dump "${args[@]}" --dump "$out/mgba.png")
    ours=$(target/release/gbaa-desktop "${args[@]}" --headless --timing exact --dump "$out/ours.png")
    # A known entry's issue and reason, and both hashes when it was listed.
    entry() { awk -F'\t' -v n="$name" -v f={{frames}} "\$1 == n && \$2 == f { print $1 }" crates/mgba-dump/known-mismatches.tsv; }
    known=$(entry '$5 " " $6')
    expect=$(entry '"frame " $2 " hash " $3 " / frame " $2 " hash " $4')
    if [ "$theirs" = "$ours" ]; then
        rm -rf "$out"
        if [ -n "$known" ]; then
            echo "$name: matches mGBA ($ours) but is listed as a known mismatch ($known): remove its line from crates/mgba-dump/known-mismatches.tsv"
            exit 1
        fi
        echo "$name: matches mGBA ($ours)"
        exit 0
    fi
    pixels=$(target/release/png-diff "$out/mgba.png" "$out/ours.png" "$out/diff.png" || true)
    if [ -n "$known" ] && [ "$theirs / $ours" = "$expect" ]; then
        echo "$name: known mismatch, $pixels ($known; $out/diff.png)"
        exit 0
    fi
    if [ -n "$known" ]; then
        echo "$name: known mismatch ($known), but a frame changed: mgba / ours were $expect when listed, now $theirs / $ours ($out/{mgba,ours,diff}.png)"
        exit 1
    fi
    echo "$name: differs from mGBA, $pixels (mgba $theirs, ours $ours; $out/{mgba,ours,diff}.png)"
    exit 1

# mgba-diff on milestone 3's ten ROMs: jsmolka's five tests at 300 frames, the three ppu tests at 60, june's first-frame ROM at 60 and the agb sprite example at 120. Fails if any fails. Needs `just fetch-tests` and `just agb-build`'s first-frame ROM.
mgba-oracle:
    #!/usr/bin/env bash
    set -u
    t={{quote(gbaa_data / "gba-tests")}}
    roms=("$t"/{arm/arm,thumb/thumb,memory/memory,bios/bios,nes/nes}.gba:300
        "$t"/ppu/{hello,shades,stripes}.gba:60
        {{quote(gbaa_data / "agb/gba-cart1.gba")}}:60
        {{quote(gbaa_data / "tools/obj-sprites/obj-sprites.gba")}}:120)
    status=0
    for r in "${roms[@]}"; do
        just mgba-diff "${r%:*}" "${r##*:}" || status=1
    done
    exit $status

# The sound oracle (#63): runs ROM for FRAMES frames (from STATE, an mGBA save state, if given) in mGBA and our interpreter, and fails unless the sound FIFOs' records (--fifo-dump) and the PSG's samples (--psg-dump) are byte-identical. On a mismatch prints the first differing record, its offset from our nearest buffer swap, and mGBA's shift if its stream is ours delayed. From reset agb games mismatch by such a shift (no memory wait states in ours, so their boot runs fast); compare from a state. Writes out/mgba-audio-diff/<name>/{mgba,ours}.{fifo,psg,log}.
mgba-audio-diff rom frames state="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    [ -n "$name" ] || { echo "no name for $rom" >&2; exit 2; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump -p desktop
    out=out/mgba-audio-diff/"$name"
    rm -rf "$out"
    mkdir -p "$out"
    args=("$rom" --frames {{frames}})
    [ -z {{quote(state)}} ] || args+=(--state {{quote(join(invocation_directory(), state))}})
    target/release/mgba-dump "${args[@]}" --fifo-dump "$out/mgba.fifo" --psg-dump "$out/mgba.psg" --audio-log "$out/mgba.log"
    target/release/gbaa-desktop "${args[@]}" --headless --timing exact --fifo-dump "$out/ours.fifo" --psg-dump "$out/ours.psg" \
        --audio-log "$out/ours.log" --audio-hash > "$out/ours.txt" 2> "$out/ours.err" || { cat "$out/ours.err"; exit 1; }
    grep -qx 'audio dropped 0' "$out/ours.txt" || { echo "FAIL: ours dropped samples ($out/ours.txt)"; exit 1; }
    echo -n "$name: "
    python3 -I scripts/audio-diff.py "$out"

# How loud we are next to mGBA (volume/measure-level): runs ROM to frame TO in mGBA and our interpreter, with --input INPUT's presses, writes each one's sound as a WAV (mGBA's mixed output at its default 100% volume; ours as `--wav`) to out/audio-loudness/<name>/{mgba,ours}.wav, and prints the peak, RMS and LUFS of frames FROM to TO of each, and ours minus mGBA's. The numbers: "Playback" in docs/sound-design.md.
audio-loudness rom from to input="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    name=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    [ -n "$name" ] || { echo "no name for $rom" >&2; exit 2; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump -p desktop
    out=out/audio-loudness/"$name"
    rm -rf "$out"
    mkdir -p "$out"
    args=("$rom" --frames {{to}})
    [ -z {{quote(input)}} ] || args+=(--input {{quote(input)}})
    target/release/mgba-dump "${args[@]}" --wav "$out/mgba.wav"
    target/release/gbaa-desktop "${args[@]}" --headless --timing exact --wav "$out/ours.wav" > "$out/ours.txt" 2> "$out/ours.err" || { cat "$out/ours.err"; exit 1; }
    target/release/loudness "$out/mgba.wav" "$out/ours.wav" --frames {{from}}..{{to}}

# mgba-dump linked to BizHawk 2.9's mGBA (TASEmulators' fork at 645282a; `scripts/fetch-mgba.sh tas`), for replaying its movies (`--bk2`; assets/fuzz-tas-bios), built in target/mgba-tas so target/release keeps the oracle's 0.10.5. Runs it with ARGS (docs/tools/commands.md, TAS replay).
[positional-arguments]
mgba-dump-tas *args:
    #!/usr/bin/env bash
    set -eu
    scripts/fetch-mgba.sh tas > /dev/null
    GBAA_MGBA=mgba-tas {{lock}} cargo build -q --release -p mgba-dump --target-dir target/mgba-tas
    target/mgba-tas/release/mgba-dump "$@"

# The PSG square channels' check (sound/psg-square): `mgba-audio-diff` on agb/psg-square (built by `just agb-tests`; its SOUND.md) for FRAMES frames from mGBA's state after frame 1, before any note. The last V-blank must lie in the first half of a 1024-cycle sampling period (119 does), or ours ends one PSG sample later (SOUND.md).
psg-square frames="119":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(gbaa_data / "agb/psg-square.gba")}}
    [ -e "$rom" ] || { echo "no $rom: run just agb-tests" >&2; exit 2; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump
    mkdir -p out/psg-square
    target/release/mgba-dump "$rom" --frames 1 --save-state out/psg-square/frame1.ss
    just mgba-audio-diff "$rom" {{frames}} out/psg-square/frame1.ss

# The PSG wave and noise channels' check (sound/psg-wave-noise): `mgba-audio-diff` on agb/psg-wave-noise (built by `just agb-tests`; its SOUND.md) for FRAMES frames from mGBA's state after frame 1, before any note. FRAMES as `just psg-square`'s.
psg-wave-noise frames="119":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(gbaa_data / "agb/psg-wave-noise.gba")}}
    [ -e "$rom" ] || { echo "no $rom: run just agb-tests" >&2; exit 2; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump
    mkdir -p out/psg-wave-noise
    target/release/mgba-dump "$rom" --frames 1 --save-state out/psg-wave-noise/frame1.ss
    just mgba-audio-diff "$rom" {{frames}} out/psg-wave-noise/frame1.ss

# Files on disk with no source or copy in git: how many, and which (exit 1 if any).
rebuildable:
    #!/usr/bin/env bash
    set -eu
    {{lock}} cargo build -q --release -p route-check
    missing=0
    GBAA_DATA={{quote(gbaa_data)}} target/release/route-check --missing units/*.toml || missing=1
    untracked=$(git ls-files --others --exclude-standard)
    [ -z "$untracked" ] || { echo "untracked (commit or ignore before landing):"; echo "$untracked"; }
    [ "$missing" -eq 0 ]

# One factory unit's check (docs/factory.md): `route-check` (crates/route-check) on units/NAME.toml, a route: runs its ROM once each in mGBA, our interpreter and the recompiled build to its last checkpoint (or THROUGH, a checkpoint's name), from empty outputs in out/units/NAME/, and fails at the first checkpoint, by name, where mGBA doesn't draw its hash (or its known-wrong mgba_hash), our interpreter doesn't, or the recompiled build's frames, registers or compiled share part from the interpreter's. Exit 75: this machine couldn't run it. Heavy: builds a generated crate.
unit name through="":
    #!/usr/bin/env bash
    set -eu
    scripts/fetch-mgba.sh > /dev/null || { echo "unit {{name}}: could not run: fetch-mgba.sh failed"; exit 75; }
    {{lock}} cargo build -q --release -p mgba-dump -p desktop -p route-check \
        || { echo "unit {{name}}: could not run: building the runners failed"; exit 75; }
    through=(); [ -z {{quote(through)}} ] || through=(--through {{quote(through)}})
    GBAA_DATA={{quote(gbaa_data)}} {{cap}} MemoryMax=8G target/release/route-check units/{{quote(name)}}.toml "${through[@]}"

# Our own save states (runtime::snap): runs ROM N+K frames with INPUT (whose presses must end by frame N), saving our state after frame N, then K frames from that state; fails unless frames N+1..N+K hash the same both ways, in the interpreter and the recompiled build. FLAGS go to gbaa-recompile ("--readable mods/emerald/readable/all.list": every function readable, mod/readable-exact), building out/<game>-<flags>. Writes out/own-state-check/<game>[-<flags>]-<n>/. Heavy: builds the game's crate.
own-state-check rom n k input="" flags="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    n={{quote(n)}} k={{quote(k)}} input={{quote(input)}} flags={{quote(flags)}}
    game=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    # A build with FLAGS gets its own crate, out/<game>-<FLAGS as letters and dashes>.
    [ -z "$flags" ] || game="$game-$(echo "$flags" | tr -cs 'a-z0-9' '-' | sed -E 's/^-+|-+$//g')"
    out=out/own-state-check/"$game-$n"
    rm -rf "$out" && mkdir -p "$out"
    fail() { echo "own-state-check $game: FAIL: $*"; exit 1; }
    {{lock}} cargo build -q --release -p desktop
    just recompile "$rom" out/"$game" --timing exact $flags > "$out/recompile.txt" 2>&1 || { tail "$out/recompile.txt"; fail "recompiling failed"; }
    just build-recompiled out/"$game"
    cap=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0)
    for build in interp recompiled; do
        bin=target/release/gbaa-desktop
        [ $build = interp ] || bin=target/recompiled/release/gbaa-"$game"
        state="$out/$build-at-$n.state"
        first=("$rom" --headless --timing exact --frames $((n + k)) --state-out "$state" --state-at "$n" --frame-hashes "$out/$build-straight.txt")
        [ -z "$input" ] || first+=(--input "$input")
        "${cap[@]}" "$bin" "${first[@]}" > /dev/null 2> "$out/$build-straight.log" \
            || { cat "$out/$build-straight.log"; fail "$build: the first run exited with an error"; }
        "${cap[@]}" "$bin" "$rom" --headless --timing exact --frames "$k" --state "$state" --frame-hashes "$out/$build-resumed.txt" \
            > /dev/null 2> "$out/$build-resumed.log" || { cat "$out/$build-resumed.log"; fail "$build: the run from the state exited with an error"; }
        tail -n "$k" "$out/$build-straight.txt" > "$out/$build-want.txt"
        awk -v n="$n" '{ $2 += n; print }' "$out/$build-resumed.txt" > "$out/$build-got.txt"
        diff "$out/$build-want.txt" "$out/$build-got.txt" > "$out/$build.diff" \
            || fail "$build: from the state, $(grep -c '^>' "$out/$build.diff") of $k frames differ; first: $(grep -m1 '^>' "$out/$build.diff")"
        echo "$build: frames $((n + 1))-$((n + k)) match ($(sort -u "$out/$build-want.txt" | awk '{print $4}' | sort -u | wc -l) distinct frames)"
    done
    cmp -s "$out/interp-want.txt" "$out/recompiled-want.txt" || echo "note: the interpreter and the recompiled build draw different frames (not this check's concern)"
    echo "own-state-check $game: pass (state at $n, $(stat -c %s "$out/recompiled-at-$n.state") bytes; $k frames from it hash as straight through)"

# Rewind (desktop::rewind): runs ROM N+K frames with INPUT straight through, then again holding rewind for K frames after frame N (back to N-K) and running on to N+K; fails unless every frame the second run shows, the rewound ones included, hashes as that frame did straight through, in the interpreter and the recompiled build. Writes out/rewind-check/<game>-<n>/. Heavy: builds the game's crate.
rewind-check rom n k input="":
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    n={{quote(n)}} k={{quote(k)}} input={{quote(input)}}
    game=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    out=out/rewind-check/"$game-$n"
    rm -rf "$out" && mkdir -p "$out"
    fail() { echo "rewind-check $game: FAIL: $*"; exit 1; }
    [ "$k" -le "$n" ] || fail "can't rewind $k frames from frame $n"
    {{lock}} cargo build -q --release -p desktop
    just recompile "$rom" out/"$game" --timing exact > "$out/recompile.txt" 2>&1 || { tail "$out/recompile.txt"; fail "recompiling failed"; }
    just build-recompiled out/"$game"
    cap=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0)
    for build in interp recompiled; do
        bin=target/release/gbaa-desktop
        [ $build = interp ] || bin=target/recompiled/release/gbaa-"$game"
        args=("$rom" --headless --timing exact --frames $((n + k)))
        [ -z "$input" ] || args+=(--input "$input")
        "${cap[@]}" "$bin" "${args[@]}" --frame-hashes "$out/$build-straight.txt" > /dev/null 2> "$out/$build-straight.log"             || { cat "$out/$build-straight.log"; fail "$build: the straight run exited with an error"; }
        "${cap[@]}" "$bin" "${args[@]}" --rewind "$n+$k" --frame-hashes "$out/$build-rewound.txt" > /dev/null 2> "$out/$build-rewound.log"             || { cat "$out/$build-rewound.log"; fail "$build: the rewound run exited with an error"; }
        lines=$(wc -l < "$out/$build-rewound.txt")
        [ "$lines" -eq $((n + 3 * k)) ] || fail "$build: the rewound run showed $lines frames, not $((n + 3 * k))"
        bad=$(awk 'NR == FNR { h[$2] = $4; next } h[$2] != $4 { print; exit }' "$out/$build-straight.txt" "$out/$build-rewound.txt")
        [ -z "$bad" ] || fail "$build: frame $(echo "$bad" | cut -d' ' -f2) after rewinding draws $(echo "$bad" | cut -d' ' -f4), straight through $(grep "^frame $(echo "$bad" | cut -d' ' -f2) " "$out/$build-straight.txt" | cut -d' ' -f4)"
        echo "$build: $lines frames shown, all as straight through ($(sed -n "$((n + 1)),$((n + k))p" "$out/$build-straight.txt" | awk '{print $4}' | sort -u | wc -l) distinct in the $k after $n); $(grep -o 'rewind: .*' "$out/$build-rewound.log")"
    done
    echo "rewind-check $game: pass (rewound $k frames at $n, ran on to $((n + k)))"

# Save-state loading (decision 16): saves ROM's state in mGBA at frame N, then fails unless K frames from it draw what N+K frames from reset do, in mGBA and in our interpreter (which must agree with mGBA at N+K), and the recompiled build from the state matches the interpreter's frame and registers. Writes out/savestate-check/<name>/. Heavy: builds the game's crate.
savestate-check rom n k:
    #!/usr/bin/env bash
    set -eu
    rom={{quote(join(invocation_directory(), rom))}}
    n={{quote(n)}} k={{quote(k)}}
    game=$(basename "$rom" .gba | tr 'A-Z' 'a-z' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g')
    out=out/savestate-check/"$game"
    rm -rf "$out" && mkdir -p "$out"
    fail() { echo "savestate-check $game: FAIL: $*"; exit 1; }
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo build -q --release -p mgba-dump -p desktop
    cap=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0)
    state="$out/at-$n.ss"
    "${cap[@]}" target/release/mgba-dump "$rom" --frames "$n" --save-state "$state" > /dev/null
    hash() { "${cap[@]}" "$@" --hash | head -1 | cut -d' ' -f4; }
    straight=$(hash target/release/mgba-dump "$rom" --frames $((n + k)))
    resumed=$(hash target/release/mgba-dump "$rom" --frames "$k" --state "$state")
    [ "$straight" = "$resumed" ] || fail "mGBA from the state draws $resumed, straight through $straight"
    ours=$(hash target/release/gbaa-desktop "$rom" --headless --timing exact --frames $((n + k)))
    [ "$ours" = "$straight" ] || fail "our interpreter draws $ours at frame $((n + k)) and mGBA $straight: pick a ROM and frames they agree on"
    args=("$rom" --headless --timing exact --frames "$k" --state "$state" --hash --regs)
    "${cap[@]}" target/release/gbaa-desktop "${args[@]}" --dump "$out/interp.png" > "$out/interp.txt" \
        || fail "the interpreter exited with an error ($(tail -1 "$out/interp.txt"))"
    from=$(head -1 "$out/interp.txt" | cut -d' ' -f4)
    [ "$from" = "$straight" ] || fail "our interpreter from the state draws $from, straight through $straight"
    just recompile "$rom" out/"$game" --timing exact > "$out/recompile.txt" 2>&1 || { tail "$out/recompile.txt"; fail "recompiling failed"; }
    just build-recompiled out/"$game"
    "${cap[@]}" target/recompiled/release/gbaa-"$game" "${args[@]}" --dump "$out/recompiled.png" \
        > "$out/recompiled.txt" 2> "$out/recompiled.log" || { cat "$out/recompiled.log"; fail "the recompiled build exited with an error"; }
    diff "$out/interp.txt" "$out/recompiled.txt" || fail "recompiled from the state differs from the interpreter (< interp, > recompiled)"
    cmp -s "$out/interp.png" "$out/recompiled.png" || fail "the recompiled PNG differs from the interpreter's"
    echo "savestate-check $game: pass (state at $n, then $k frames: hash $straight in mGBA, the interpreter and recompiled, as $((n + k)) from reset)"

# The Lua host's check: the fixture in crates/script/fixtures/emerald/ and mGBA 0.10.5's res/scripts/pokemon.lua (the pinned checkout, scripts/fetch-mgba.sh) run headless on Emerald (USA) from power-on with no save; fails unless the scripts' console and buffers match the fixture's expected.txt (crates/desktop/tests/script_emerald.rs); and mGBA's res/scripts/socketserver.lua, headless on a looping test ROM, must accept a loopback client, log what it sends and send it the keys when A is pressed (crates/desktop/tests/script_sockets.rs, about 10 s); and crates/script/fixtures/breakpoints/ (breakpoints and watchpoints at their frames, crates/desktop/tests/script_breakpoints.rs), run by gbaa-desktop, or by BIN (a recompiled build, `just build-recompiled`) if given; and mGBA master's canvas demos text-demo.lua and logo-bounce.lua (fetched into $GBAA_DATA/tools/mgba-demos/) must draw their labels and a moving logo over Emerald in --dump-at's PNGs (crates/desktop/tests/script_canvas.rs, gbaa-desktop only).
script-check rom=(roms / "Pokemon - Emerald Version.gba") bin="":
    scripts/fetch-mgba.sh > /dev/null
    {{lock}} cargo test -q --release -p desktop --test script_emerald --test script_sockets --test script_breakpoints --test script_canvas --no-run
    GBAA_SCRIPT_ROM={{quote(rom)}} GBAA_SCRIPT_MGBA={{quote(gbaa_data / "tools/mgba/src")}} GBAA_SCRIPT_DEMOS={{quote(gbaa_data / "tools/mgba-demos")}} GBAA_SCRIPT_BIN={{quote(if bin == "" { "" } else { absolute_path(bin) })}} cargo test -q --release -p desktop --test script_emerald --test script_sockets --test script_breakpoints --test script_canvas -- --ignored

# Two Emerald saves for link tests (multiplayer/cable-club-saves): runs ROM to the new game's truck in the interpreter, patches its save blocks into two trainers (ALICE: Mudkip, Wurmple; BOB: Torchic, Zigzagoon) standing below Oldale Town's cable club attendant, and writes $GBAA_DATA/saves/emerald/{alice,bob}.sav (crates/emerald-save). About 20 s; the same ROM gives the same files.
emerald-saves rom=(roms / "Pokemon - Emerald Version.gba"):
    {{lock}} cargo build -q --release -p emerald-save
    target/release/emerald-save {{quote(rom)}} {{quote(gbaa_data / "saves/emerald")}}

# Story-point saves for the fuzzer (assets/fuzz-story-seeds): alice.sav (`just emerald-saves`) moved along the main story by crates/emerald-save/story.toml's points (flags, vars, key items and HMs as pret's scripts leave them, the party's level, where the player stands), each point with every one before it applied; names resolved against pret's headers (`just fetch-pret`). Writes $GBAA_DATA/saves/emerald/story/NN-name.sav (crates/emerald-save/src/bin/emerald-story.rs). Seconds.
emerald-story rom=(roms / "Pokemon - Emerald Version.gba"):
    {{lock}} cargo build -q --release -p emerald-save
    target/release/emerald-story {{quote(rom)}} {{quote(gbaa_data / "src/pokeemerald")}} crates/emerald-save/story.toml {{quote(gbaa_data / "saves/emerald/alice.sav")}} {{quote(gbaa_data / "saves/emerald/story")}}

# A TAS's story states for the fuzzer (assets/fuzz-tas-bios): drkspace's glitchless living dex (TASVideos movie 6182, BizHawk 2.9), fetched once to $GBAA_DATA/tas/6182.zip and checked against its hash, replayed by `just mgba-dump-tas` (BizHawk's mGBA, on the official BIOS: $GBAA_DATA/bios/gba_bios.bin) with a state every EVERY frames into $GBAA_DATA/saves/emerald/story/state-N.ss, beside `just emerald-story`'s saves, then listed by `gbaa-fuzz story` (badges should only rise). That folder, as `just fuzz-coverage`'s STORY, seeds the workers with both. The whole movie (3,094,273 frames) takes about half an hour.
emerald-tas every="100000" rom=(roms / "Pokemon - Emerald Version.gba"):
    #!/usr/bin/env bash
    set -eu
    bk2={{quote(gbaa_data / "tas/6182.zip")}}
    bios={{quote(gbaa_data / "bios/gba_bios.bin")}}
    out={{quote(gbaa_data / "saves/emerald/story")}}
    [ -e "$bios" ] || { echo "no $bios: the owner's BIOS dump" >&2; exit 2; }
    mkdir -p "$(dirname "$bk2")" "$out"
    [ -e "$bk2" ] || curl -sfL -o "$bk2" 'https://tasvideos.org/6182M?handler=Download'
    echo "6718d8f577108ec276473ccc6313b9a65978dfb5e3e9a3104aaa95554d5d1cac  $bk2" | sha256sum -c --quiet
    # TASVideos serves a zip holding the .bk2, itself a zip.
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
    unzip -p "$bk2" '*.bk2' > "$tmp/movie.bk2"
    log="$tmp/input.txt"
    unzip -p "$tmp/movie.bk2" 'Input Log.txt' > "$log"
    frames=$(grep -c '^|' "$log")
    rm -f "$out"/state-*.ss
    just mgba-dump-tas {{quote(rom)}} --frames "$frames" --bk2 "$log" --bios "$bios" --save-state-at "$(seq -s, {{every}} {{every}} "$frames")" --state-dir "$out"
    {{lock}} cargo build -q --release -p fuzz
    target/release/gbaa-fuzz story "$out"

# The link hub's check (multiplayer/link-hub): the two saves (just emerald-saves) as two Emeralds on one cable (runtime::link), in the interpreter, through the cable club to the Trade Center; passes when both reach LINK_STATE_CONN_ESTABLISHED and the Trade Center, identically twice (crates/emerald-save/src/bin/emerald-link.rs). About a minute. ARGS: --frames, --input0/--input1, --dump DIR --dump-at N,N.
emerald-link rom=(roms / "Pokemon - Emerald Version.gba") *args:
    {{lock}} cargo build -q --release -p emerald-save
    target/release/emerald-link {{quote(rom)}} {{quote(gbaa_data / "saves/emerald")}} {{args}}

# The netplay session core's check (multiplayer/session-core): the two saves as two linked Emeralds walking into the Trade Center, straight and as a two-peer rollback session at no input delay over a simulated network (40 ms, +30 ms jitter, 10% loss); passes when both peers' hashes every 60 frames and at the end are the straight run's, with no desync (crates/emerald-save/src/bin/emerald-netplay.rs). About 90 s. ARGS: --delay, --latency, --jitter, --loss, --seed, --frames.
emerald-netplay rom=(roms / "Pokemon - Emerald Version.gba") *args:
    {{lock}} cargo build -q --release -p emerald-save
    target/release/emerald-netplay {{quote(rom)}} {{quote(gbaa_data / "saves/emerald")}} {{args}}

# The variable-ping simulator (multiplayer/ping-sim): two peer processes play a rollback netplay session over UDP through a proxy that delays, loses, reorders and duplicates as a profile says (crates/netplay/profiles/NAME.toml: calm, wander, spiky, rough); passes when both peers' hashes every 60 frames and at the end are a straight run's (crates/netplay/src/bin/gbaa-pingsim.rs). Real time: 600 frames at 60 fps, about 15 s. ARGS: --console toy|gba, --frames, --delay, --fps, --linger-ms.
ping-sim profile="rough" *args:
    {{lock}} cargo build -q --release -p netplay --bin gbaa-pingsim
    target/release/gbaa-pingsim run crates/netplay/profiles/{{profile}}.toml {{args}}

# The server's check (multiplayer/relay-server): two peer processes play the toy game by rollback through gbaa-server's rooms (a local one, in-process), each peer's path to the server through a proxy following the profile (as ping-sim's), seat 0 making the room and seat 1 joining by its code; passes as ping-sim does. About 15 s. --server ADDR plays through that server instead (fly's: --server 37.16.3.41, after fly-up). ARGS as ping-sim's.
room-sim profile="calm" *args:
    {{lock}} cargo build -q --release -p netplay --bin gbaa-pingsim
    target/release/gbaa-pingsim room crates/netplay/profiles/{{profile}}.toml {{args}}

# Rollback, felt (multiplayer/rollback-design): two gbaa-desktop netplay windows on ROM through a local server, the joiner's path through the profile's proxy (crates/netplay/profiles/NAME.toml: calm, wander, spiky, rough), so the profile is the whole trip between the players. The keyboard drives the window with focus; a gamepad drives both. Add --headless --paced --frames N --input1 S --input2 S for numbers instead (each peer's end line: rollbacks, frames run again, freezes, frame times). ARGS: --delay D (0 by default), --save1/--save2 F, --menu1/--menu2 SPANS (headless: when each seat's menu is open, as frame[+frames] or from-to/every+frames).
rollback-feel profile="wander" rom=(roms / "Kirby & The Amazing Mirror.gba") *args:
    {{lock}} cargo build -q --release -p netplay --bin gbaa-pingsim -p desktop --bin gbaa-desktop
    target/release/gbaa-pingsim desktop crates/netplay/profiles/{{profile}}.toml {{quote(rom)}} -- {{args}}

# gbaa-server on fly.io (multiplayer/play-gate; deploy/fly/): the app gbaa-server in ewr, one shared-cpu machine, its rooms on UDP 7374. It keeps no files, so no volume. fly-setup once (billed: a dedicated IPv4, which public UDP needs), then fly-deploy after any server change.
fly-setup:
    fly ips list -a gbaa-server | grep -q 'v4.*dedicated' || fly ips allocate-v4 -a gbaa-server -y

# --ha=false: rooms live in one process's memory, so a second machine behind the same IPv4 could split a room's players. Without it, a deploy onto an app with no machines makes two.
fly-deploy:
    fly deploy --config deploy/fly/fly.toml --remote-only --ha=false .

# The fly machine is stopped between june's playtests (june, 2026-10-27): fly-up before playing or before a unit's test that needs it, fly-down after. Stopped, the IPv4 is still billed and kept, so the address stays 37.16.3.41.
fly-up:
    fly machine list -a gbaa-server -q | xargs -r fly machine start -a gbaa-server

fly-down:
    fly machine list -a gbaa-server -q | xargs -r fly machine stop -a gbaa-server

# Linked play's check (multiplayer/local-link-window): copies of the two saves as two linked Emeralds in gbaa-desktop --headless --link, through the cable club to a trade of their first Pokémon; passes when the saves the desktop wrote back hold the trade (crates/emerald-save/src/bin/emerald-trade.rs). About a minute. ARGS: --keep DIR.
emerald-trade rom=(roms / "Pokemon - Emerald Version.gba") *args:
    {{lock}} cargo build -q --release -p emerald-save -p desktop
    target/release/emerald-trade target/release/gbaa-desktop {{quote(rom)}} {{quote(gbaa_data / "saves/emerald")}} {{args}}

# Netplay's check (multiplayer/desktop-netplay): the same trade between two gbaa-desktop processes, Alice's hosting a room and Bob's joining it by code, by rollback at no input delay through a server's plain-UDP rooms started in-process, each path to it through the variable-ping simulator's PROFILE (crates/netplay/profiles/NAME.toml); passes when both saves hold the trade and both players' consoles end on the same hash, no desync (crates/emerald-save/src/bin/emerald-trade.rs --netplay). About two minutes with calm or rough. ARGS: --keep DIR.
emerald-netplay-trade profile="calm" rom=(roms / "Pokemon - Emerald Version.gba") *args:
    {{lock}} cargo build -q --release -p emerald-save -p desktop
    target/release/emerald-trade target/release/gbaa-desktop {{quote(rom)}} {{quote(gbaa_data / "saves/emerald")}} --netplay --profile crates/netplay/profiles/{{profile}}.toml {{args}}

# The hole report (`docs/research/native-holes.md`): what blocks a native build of ROM. Discovery, a 5,400-frame interpreter run with the research press script, the function summaries; prints the ranking line and the holes by kind. About a minute a game; the run is capped at 3 GB of RAM and no swap.
[positional-arguments]
holes rom *args:
    {{lock}} cargo build -q --release -p hole-report
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/release/gbaa-holes "$@"

# Emerald's ten native functions against the play build (native/emerald-ten; scores in docs/native-port-design.md, "Emerald's ten"): the hand-lifted Rust in mods/emerald/native-ten/ run by crates/native-fn-diff from 600 no-input frames' entry states (borrowed for functions not entered) in 96 variants each, against the interpreter untimed; then one byte of Random's literal pool changed must be caught. Fails unless it prints `native 0 of 10 differ` and the fault is caught. ROM defaults to pret's build. About 30 s (the function summaries are most of it), capped at 3 GB of RAM and no swap. Private: mods/ isn't in the public mirror.
[positional-arguments]
native-ten rom=(gbaa_data / "src/pokeemerald/pokeemerald.gba"):
    {{lock}} cargo build -q --release -p native-fn-diff --features emerald
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 target/release/native-ten "$1"
