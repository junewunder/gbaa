//! `gbaa-assets`, the asset exporter (docs/asset-extraction-design.md):
//!
//! - `gbaa-assets songs ROM M4A_LOG OUT [--names MAP]`: the song table found
//!   from the first song header `M4A_LOG` (`gbaa-interp --m4a-log`) saw
//!   played, every song in it decoded and written as `OUT/songs/<name>.mid`,
//!   and `OUT/manifest.toml`'s `[songs]`. Names from the source map `MAP`
//!   (pret's symbols), else `song_<index>`.
//! - `gbaa-assets samples ROM OUT [--names MAP]`: after `songs`, each
//!   song's voicegroup (and the keysplit and drum-set groups in it) read to
//!   what its songs play, and the ROM's cry tables (found by shape):
//!   `OUT/samples/<name>.wav` and `.pcm`, `OUT/voicegroups/<name>.toml`,
//!   `OUT/soundfonts/<voicegroup>.sf2`, and the
//!   manifest's `[samples]` and `[voicegroups]`.
//! - `gbaa-assets check-pret samples ROM OUT PRET [--min-exact N]`: each
//!   `.wav` through pret's wav2agb against pret's `.bin` and the ROM's
//!   bytes, each `.pcm` against pret's.
//! - `gbaa-assets check-pret songs ROM OUT PRET [--min-exact N]
//!   [--min-events N]`: each song through pret's mid2agb against pret's
//!   built `.s`, written to `OUT/pret-check.txt` and summed up; fails below
//!   the floors, or if pret's own `.s` doesn't assemble to the ROM's bytes.
//! - `gbaa-assets check-load-log ROM DIR [--names MAP]`: a recording run's
//!   load and use logs in `DIR` (`gbaa-desktop --assets-log`) against what
//!   reached the screen and what its last frame showed
//!   ([`assets::loadlog`]), lengths against the source map `MAP`'s sizes;
//!   written to `DIR/check.txt`, and fails if any blob, range or tile is
//!   missed.
//! - `gbaa-assets graphics ROM OUT [--names MAP] [--names-out FILE]`: the
//!   tile sheets, palettes and BG tilemaps every route under `OUT`
//!   (`OUT/<route>/loads.tsv` and `uses.tsv`) showed, written to
//!   `OUT/tiles`, `OUT/palettes` and `OUT/tilemaps`, and
//!   `OUT/manifest.toml`'s `[tiles]`, `[palettes]` and `[tilemaps]`
//!   ([`assets::graphics`]). `--names-out`: the source map layer naming
//!   the blobs `MAP` doesn't. Routes with an OAM log (`oam.tsv`) give the
//!   sprite sheets and animations too ([`assets::sprites`]):
//!   `OUT/sprites/*.aseprite`, `[sprites]` and `OUT/sprites.txt`.
//!   The export itself is [`assets::export::graphics`], which the desktop's
//!   `--extract` calls too.
//! - `gbaa-assets check-extract OUT SESSION [--before MANIFEST] [--min-new N]`: a
//!   `gbaa-desktop --extract OUT` session at `SESSION` against `OUT`'s
//!   export ([`assets::export::check`]): every tile and palette blob it
//!   showed has a manifest entry with its files, and every entry of
//!   `MANIFEST` (the one before the session) is kept under its name, and
//!   at least `N` entries are new; written to `SESSION/extract-check.txt`.
//! - `gbaa-assets sprite-tables ROM OUT --tables ADDR [--anim-info
//!   ADDR:N] [--anims LIST] [--palette ADDR] [--labels FILE]`: the Sonic
//!   Advance engine's sprite tables at `ADDR` ([`assets::spritetables`]):
//!   each chosen animation (those an `AnimInfo` table of `N` entries uses,
//!   and `LIST`, e.g. `1,4-7`) composed whole into
//!   `OUT/sprite-tables/anim_<id>[_<label>].aseprite` and `.png`, with
//!   `palette ADDR`'s 16 colours or the script's own; every animation's
//!   scripts in `anims.txt` and their `AnimCmd_6` hitboxes in
//!   `hitboxes.tsv`. Names from the TOML `FILE` (`[anims]`, `[variants]`).
//! - `gbaa-assets check-pret graphics ROM OUT PRET --names MAP
//!   [--min-bytes N] [--min-rebuilds N]`: each blob against pret's
//!   `INCGFX` file at four levels ([`assets::pretgfx`]), written to
//!   `OUT/pret-check-graphics.txt`; fails below the floors.
//! - `gbaa-assets check-pret sprites ROM OUT PRET --names MAP
//!   [--min-strips N]`: each sprite sheet and its Aseprite frames against
//!   pret's PNG ([`assets::sprites::check`]), written to
//!   `OUT/pret-check-sprites.txt`; fails below `N` object-event and Pokémon
//!   strips at the image level.
//! - `gbaa-assets check-overrides round-trip ROM OUT`: every exported file
//!   in `OUT` encoded back to its blob's bytes ([`assets::overrides`]),
//!   failing on any that differ, and `OUT/overrides/unedited/mod.toml`, a
//!   mod of them all.
//! - `gbaa-assets check-overrides edit OUT --sprite ADDR --sheet ADDR`:
//!   `OUT/overrides/edited/`, a mod with the Aseprite sprite at `ADDR` and
//!   the tile sheet PNG at `ADDR` edited.
//! - `gbaa-assets check-overrides frames PLAIN UNEDITED EDITED --differ
//!   N,…`: three runs' `--frame-hashes`: the unedited mod's must equal the
//!   plain run's, and the edited one's differ at each frame `N`.
//! - `gbaa-assets coverage ROM PRET --names MAP --out DIR RUN…`: pret's
//!   image, palette and tilemap files joined to the ROM, the ones never
//!   used in play left out with the reason, and each `RUN` (a fuzz run's
//!   folder, scored as its route phase and whole, or a route's
//!   `manifest.toml`) scored against the rest ([`assets::coverage`]):
//!   `DIR/coverage.txt` and `coverage.tsv`. With `--scan`, a "scan only"
//!   column: files a run missed that the static scan finds. With `--curve
//!   SECS`, `DIR/coverage-curve.tsv`: the one fuzz run's used files found
//!   every `SECS` of run time.
//! - `gbaa-assets scan ROM OUT [--run RUN]…`: every LZ77 blob in the ROM by
//!   Nintenlord's rule ([`assets::scan`]), no pret needed; the ones no
//!   `RUN` reached (a fuzz run's folder or a route's `manifest.toml`)
//!   written as greyscale sheets, `OUT/scan/*.png` and `OUT/scan/scan.toml`,
//!   marked found by scan, with no palette or layout.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use assets::m4a::{self, Header, Mem, ROM_BASE};
use assets::songs;

mod instruments;

const USAGE: &str = "usage: gbaa-assets songs ROM M4A_LOG OUT [--names MAP]\n       gbaa-assets samples ROM OUT [--names MAP]\n       gbaa-assets check-pret samples ROM OUT PRET [--min-exact N]\n       gbaa-assets check-pret songs ROM OUT PRET [--min-exact N] [--min-events N]\n       gbaa-assets check-load-log ROM DIR [--names MAP]\n       gbaa-assets graphics ROM OUT [--names MAP] [--names-out FILE]\n       gbaa-assets sprite-tables ROM OUT --tables ADDR [--anim-info ADDR:N] [--anims LIST] [--palette ADDR] [--labels FILE]\n       gbaa-assets check-pret graphics ROM OUT PRET --names MAP [--min-bytes N] [--min-rebuilds N]\n       gbaa-assets check-pret sprites ROM OUT PRET --names MAP [--min-strips N]\n       gbaa-assets check-overrides round-trip ROM OUT\n       gbaa-assets check-overrides edit OUT --sprite ADDR --sheet ADDR\n       gbaa-assets check-overrides frames PLAIN UNEDITED EDITED --differ N,…\n       gbaa-assets check-extract OUT SESSION [--before MANIFEST] [--min-new N]\n       gbaa-assets coverage ROM PRET --names MAP --out DIR [--scan] [--curve SECS] RUN…\n       gbaa-assets scan ROM OUT [--run RUN]…";

/// The song headers the log saw started, in order, each with the first
/// frame it was.
fn played(log: &str) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for line in log.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(frame), Some(header)) = (
            f.first().and_then(|v| v.parse().ok()),
            f.get(2)
                .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()),
        ) else {
            continue;
        };
        if !out.iter().any(|&(h, _)| h == header) {
            out.push((header, frame));
        }
    }
    out
}

/// A name usable as a file name.
fn file_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn export_songs(
    rom_path: &str,
    log_path: &str,
    out: &str,
    names: Option<&str>,
) -> Result<(), String> {
    let rom_bytes = std::fs::read(rom_path).map_err(|e| format!("{rom_path}: {e}"))?;
    let rom = Mem {
        base: ROM_BASE,
        bytes: &rom_bytes,
    };
    let log = std::fs::read_to_string(log_path).map_err(|e| format!("{log_path}: {e}"))?;
    let played = played(&log);
    let sha = codegen::names::sha1_hex(&rom_bytes);
    let map = match names {
        Some(p) => Some(codegen::names::SourceMap::load(&sha, &[p])?),
        None => None,
    };
    // Cries' headers are copied to RAM; the table holds ROM headers.
    let &(first, first_frame) = played
        .iter()
        .find(|(h, _)| rom.holds(*h))
        .ok_or(format!("{log_path}: no song header in ROM was played"))?;
    let table =
        m4a::find_song_table(rom, first).ok_or(format!("no song table holds {first:#010x}"))?;
    println!(
        "song table at {:#010x}: {} entries (found from {first:#010x}, played at frame {first_frame}){}",
        table.addr,
        table.entries.len(),
        if table.referenced {
            ""
        } else {
            "; no ROM word points at it"
        }
    );

    let dir = Path::new(out);
    std::fs::create_dir_all(dir.join("songs")).map_err(|e| format!("{out}: {e}"))?;
    // By header: its indices in the table.
    let mut by_header: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, e) in table.entries.iter().enumerate() {
        by_header.entry(e.header).or_default().push(i);
    }
    let mut manifest = String::new();
    let _ = writeln!(
        manifest,
        "# Written by `gbaa-assets songs` (crates/assets). Don't edit.\n\n[manifest]\nrom_sha1 = \"{sha}\"\nformat = 1\nsong_table = {:#010x}\nsong_table_entries = {}\n\n[songs]",
        table.addr,
        table.entries.len()
    );
    // The table's dummy: the header most entries share (a song with no
    // tracks of its own, as pret's `dummy_song_header`).
    let dummy = by_header
        .iter()
        .max_by_key(|(_, i)| i.len())
        .filter(|(_, i)| i.len() > 1)
        .map(|(&h, _)| h);
    let (mut written, mut dummies, mut odd, mut failed) = (0, 0, 0, Vec::new());
    for (&h, indices) in &by_header {
        let header = Header::read(rom, h).ok_or(format!("header {h:#010x} runs off the ROM"))?;
        if Some(h) == dummy {
            dummies += indices.len();
            continue;
        }
        let name = match map.as_ref().and_then(|m| m.data.get(&h)) {
            Some(item) => file_name(&item.name),
            None => format!("song_{:04}", indices[0]),
        };
        let song = match m4a::decode_song(rom, h) {
            Ok(s) => s,
            Err(e) => {
                failed.push(format!("{name} ({h:#010x}): {e}"));
                continue;
            }
        };
        let midi = songs::to_midi(&song, &name);
        let bars = songs::bars(&song);
        let file = format!("songs/{name}.mid");
        std::fs::write(dir.join(&file), midi.smf.to_bytes()).map_err(|e| format!("{file}: {e}"))?;
        written += 1;
        if !midi.odd.is_empty() {
            odd += 1;
        }
        let played_frame = played
            .iter()
            .find(|p| p.0 == h)
            .map_or(String::new(), |p| format!(", played_frame = {}", p.1));
        let odd_list = midi
            .odd
            .iter()
            .map(|o| format!("{o:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            manifest,
            "{h:#010x} = {{ index = {indices:?}, name = \"{name}\", voicegroup = {:#010x}, tracks = {}, bars = {:?}, priority = {}, reverb = {}, file = \"{file}\"{played_frame}, odd = [{odd_list}] }}",
            header.tone,
            header.tracks.len(),
            bars.segs
                .iter()
                .map(|&(at, len)| [at, len])
                .collect::<Vec<_>>(),
            header.priority,
            header.reverb,
        );
    }
    // The graphics export's sections stay.
    let old = std::fs::read_to_string(dir.join("manifest.toml")).unwrap_or_default();
    let manifest = assets::graphics::merge_manifest(&old, &["manifest", "songs"], &manifest, "");
    std::fs::write(dir.join("manifest.toml"), manifest)
        .map_err(|e| format!("{out}/manifest.toml: {e}"))?;
    println!(
        "songs: {written} written to {out}/songs ({odd} with something mid2agb can't carry), {dummies} dummy entries skipped, {} failed",
        failed.len()
    );
    for f in &failed {
        println!("  failed: {f}");
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} songs failed to decode", failed.len()))
    }
}

fn floor(args: &[String], flag: &str) -> Result<usize, String> {
    match args.iter().position(|a| a == flag) {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .ok_or(format!("{flag} needs a number")),
        None => Ok(0),
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, String> {
    match args.iter().position(|a| a == name) {
        Some(i) => args
            .get(i + 1)
            .map(|v| Some(v.as_str()))
            .ok_or(format!("{name} needs a value")),
        None => Ok(None),
    }
}

fn export_graphics(
    rom_path: &str,
    out: &str,
    names: Option<&str>,
    names_out: Option<&str>,
) -> Result<(), String> {
    use assets::graphics;
    let rom = std::fs::read(rom_path).map_err(|e| format!("{rom_path}: {e}"))?;
    let sha = codegen::names::sha1_hex(&rom);
    let data = match names {
        Some(p) => codegen::names::SourceMap::load(&sha, &[p])?.data,
        None => BTreeMap::new(),
    };
    let sizes: BTreeMap<u32, u64> = data
        .iter()
        .filter_map(|(&a, i)| Some((a, i.size?)))
        .collect();
    let named: BTreeMap<u32, String> = data.into_iter().map(|(a, i)| (a, i.name)).collect();
    let dir = Path::new(out);
    let e = assets::export::graphics(&rom, dir, &named, &sizes)?;
    println!("{}", e.summary);
    if let Some(p) = names_out {
        std::fs::write(p, graphics::names_layer(&e.found, &named, &sha))
            .map_err(|e| format!("{p}: {e}"))?;
        println!("names layer: {p}");
    }
    Ok(())
}

/// `0x…` or decimal.
fn number(s: &str) -> Result<u32, String> {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|_| format!("{s:?}: not a number"))
}

/// `gbaa-assets coverage`: see the module doc.
fn coverage(rom: &str, pret: &str, rest: &[String]) -> Result<(), String> {
    use assets::coverage as cov;
    let map = flag(rest, "--names")?.ok_or("coverage needs --names MAP")?;
    let out = flag(rest, "--out")?.ok_or("coverage needs --out DIR")?;
    let scan = rest.iter().any(|a| a == "--scan");
    let step = flag(rest, "--curve")?
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| "--curve needs seconds")?;
    let mut skip = false;
    let runs_in: Vec<&String> = rest
        .iter()
        .filter(|a| *a != "--scan")
        .filter(|a| {
            let flagged = std::mem::replace(&mut skip, a.starts_with("--"));
            !flagged && !a.starts_with("--")
        })
        .collect();
    if runs_in.is_empty() {
        return Err(USAGE.into());
    }
    let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
    let sha = codegen::names::sha1_hex(&rom_bytes);
    let data: Vec<_> = codegen::names::SourceMap::load(&sha, &[map])?
        .data
        .into_iter()
        .map(|(a, i)| (a, i.name, i.size.unwrap_or(0) as u32, i.file))
        .collect();
    let incs = cov::scan(Path::new(pret))?;
    let unscripted = cov::scan_unscripted(Path::new(pret))?;
    let files = cov::files(&incs, &data, &rom_bytes, &unscripted);
    let mut runs = Vec::new();
    for &r in &runs_in {
        let p = Path::new(r);
        if p.is_dir() {
            let (route, all) = cov::from_fuzz(p)?;
            runs.push((format!("{r} (route phase)"), cov::score(&files, &route)));
            runs.push((r.clone(), cov::score(&files, &all)));
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{r}: {e}"))?;
            runs.push((r.clone(), cov::score(&files, &cov::from_manifest(&text)?)));
        }
    }
    let mut summary = String::new();
    if scan {
        let starts: std::collections::HashSet<u32> = assets::scan::lz77_blobs(&rom_bytes)
            .iter()
            .map(|b| b.addr)
            .collect();
        for (_, st) in &mut runs {
            *st = cov::with_scan(&files, st, &starts);
        }
        let named: std::collections::HashSet<u32> = data.iter().map(|d| d.0).collect();
        summary = cov::scan_summary(&files, &named, &starts);
    }
    let named: Vec<(&str, Vec<_>)> = runs.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();
    let (mut text, tsv) = cov::report(&files, &named);
    let at = text.find("\n## ").unwrap_or(text.len());
    text.insert_str(at, &summary);
    let dir = Path::new(out);
    std::fs::create_dir_all(dir).map_err(|e| format!("{out}: {e}"))?;
    for (n, body) in [("coverage.txt", &text), ("coverage.tsv", &tsv)] {
        let p = dir.join(n);
        std::fs::write(&p, body).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    if let Some(step) = step {
        let fuzz: Vec<&&String> = runs_in.iter().filter(|r| Path::new(r).is_dir()).collect();
        let [run] = fuzz[..] else {
            return Err("--curve takes one fuzz run's folder".into());
        };
        let body = cov::curve(&files, Path::new(run), step)?;
        let p = dir.join("coverage-curve.tsv");
        std::fs::write(&p, &body).map_err(|e| format!("{}: {e}", p.display()))?;
        print!("{body}");
    }
    print!("{text}");
    println!("(per file: {})", dir.join("coverage.tsv").display());
    Ok(())
}

/// `gbaa-assets scan`: see the module doc.
fn scan(rom: &str, out: &str, rest: &[String]) -> Result<(), String> {
    use assets::coverage as cov;
    let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
    let blobs = assets::scan::lz77_blobs(&rom_bytes);
    let step = match rest.iter().position(|a| a == "--curve") {
        Some(i) => Some(
            rest.get(i + 1)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|&v| v > 0.0)
                .ok_or("--curve needs seconds")?,
        ),
        None => None,
    };
    let mut runs = Vec::new();
    for (i, a) in rest.iter().enumerate() {
        match (a.as_str(), rest.get(i + 1)) {
            ("--run", Some(r)) => {
                let p = Path::new(r);
                runs.push(if p.is_dir() {
                    let (route, all) = cov::from_fuzz(p)?;
                    let n = |f: &cov::Found| {
                        blobs.iter().filter(|b| assets::scan::reached(f, b)).count()
                    };
                    println!(
                        "{r}: its route phase reached {} blobs, the whole run {}",
                        n(&route),
                        n(&all)
                    );
                    if let Some(step) = step {
                        let text = assets::scan::curve(&blobs, p, step)?;
                        let out = p.join("scan-curve.tsv");
                        std::fs::write(&out, text)
                            .map_err(|e| format!("{}: {e}", out.display()))?;
                    }
                    all
                } else {
                    let text = std::fs::read_to_string(p).map_err(|e| format!("{r}: {e}"))?;
                    cov::from_manifest(&text)?
                });
            }
            ("--run", None) => return Err("--run needs a value".into()),
            _ => {}
        }
    }
    let left: Vec<_> = blobs
        .iter()
        .filter(|b| !runs.iter().any(|f| assets::scan::reached(f, b)))
        .copied()
        .collect();
    assets::scan::write(&rom_bytes, &left, Path::new(out))?;
    let bytes: u64 = left.iter().map(|b| b.size as u64).sum();
    println!(
        "{} LZ77 blobs; {} reached by the {} run(s); {} not, written to {}/scan/ ({} KiB unpacked)",
        blobs.len(),
        blobs.len() - left.len(),
        runs.len(),
        left.len(),
        out,
        bytes / 1024
    );
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    use assets::overrides;
    match args {
        [cmd, kind, rom, out] if cmd == "check-overrides" && kind == "round-trip" => {
            let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
            let out = Path::new(out);
            let (text, failed) =
                overrides::round_trip(&rom_bytes, out, &out.join("overrides/unedited"))?;
            print!("{text}");
            match failed {
                0 => Ok(()),
                n => Err(format!(
                    "{n} exported files don't encode back to their bytes"
                )),
            }
        }
        [cmd, kind, out, rest @ ..] if cmd == "check-overrides" && kind == "edit" => {
            let sprite = number(flag(rest, "--sprite")?.ok_or(USAGE)?)?;
            let sheet = number(flag(rest, "--sheet")?.ok_or(USAGE)?)?;
            let out = Path::new(out);
            let text = overrides::edited_mod(out, &out.join("overrides/edited"), sprite, sheet)?;
            println!("{text}");
            Ok(())
        }
        [cmd, kind, plain, unedited, edited, rest @ ..]
            if cmd == "check-overrides" && kind == "frames" =>
        {
            let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
            let differ = flag(rest, "--differ")?
                .ok_or(USAGE)?
                .split(',')
                .map(|n| number(n).map(u64::from))
                .collect::<Result<Vec<_>, _>>()?;
            let text =
                overrides::check_frames(&read(plain)?, &read(unedited)?, &read(edited)?, &differ)?;
            print!("{text}");
            Ok(())
        }
        [cmd, rom, log, out, rest @ ..] if cmd == "songs" => {
            let names = match rest {
                [] => None,
                [f, map] if f == "--names" => Some(map.as_str()),
                _ => return Err(USAGE.into()),
            };
            export_songs(rom, log, out, names)
        }
        [cmd, rom, out, rest @ ..] if cmd == "samples" => {
            let names = match rest {
                [] => None,
                [f, map] if f == "--names" => Some(map.as_str()),
                _ => return Err(USAGE.into()),
            };
            instruments::export(rom, out, names)
        }
        [cmd, kind, rom, out, pret, rest @ ..] if cmd == "check-pret" && kind == "samples" => {
            let min_exact = floor(rest, "--min-exact")?;
            let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
            let r = assets::pret::check_samples(&rom_bytes, Path::new(out), Path::new(pret))?;
            let path = Path::new(out).join("pret-check-samples.txt");
            std::fs::write(&path, r.text()).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("{}(per sample: {})", r.summary(), path.display());
            if r.exact < min_exact {
                return Err(format!(
                    "below the floor: {} samples exact (at least {min_exact})",
                    r.exact
                ));
            }
            Ok(())
        }
        [cmd, kind, rom, out, pret, rest @ ..] if cmd == "check-pret" && kind == "songs" => {
            let (min_exact, min_events) =
                (floor(rest, "--min-exact")?, floor(rest, "--min-events")?);
            let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
            let r = assets::pret::check_songs(&rom_bytes, Path::new(out), Path::new(pret))?;
            let path = Path::new(out).join("pret-check.txt");
            std::fs::write(&path, r.text()).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("{}(per song: {})", r.summary(), path.display());
            if r.pret_rom < r.exported {
                return Err(format!(
                    "pret's .s assembles to the ROM's bytes for only {} of {}: the assembler is wrong",
                    r.pret_rom, r.exported
                ));
            }
            if r.exact < min_exact || r.events < min_events {
                return Err(format!(
                    "below the floor: {} exact (at least {min_exact}), {} equal as events (at least {min_events})",
                    r.exact, r.events
                ));
            }
            Ok(())
        }
        [cmd, rom, pret, rest @ ..] if cmd == "coverage" => coverage(rom, pret, rest),
        [cmd, rom, out, rest @ ..] if cmd == "scan" => scan(rom, out, rest),
        [cmd, kind, rom, out, pret, rest @ ..] if cmd == "check-pret" && kind == "graphics" => {
            let map = flag(rest, "--names")?.ok_or("check-pret graphics needs --names MAP")?;
            let (min_bytes, min_rebuilds) =
                (floor(rest, "--min-bytes")?, floor(rest, "--min-rebuilds")?);
            let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
            let sha = codegen::names::sha1_hex(&rom_bytes);
            let m = codegen::names::SourceMap::load(&sha, &[map])?;
            let names = m
                .data
                .into_iter()
                .map(|(a, i)| (a, (i.name, i.file)))
                .collect();
            let r = assets::pretgfx::check(&rom_bytes, Path::new(out), Path::new(pret), &names)?;
            let path = Path::new(out).join("pret-check-graphics.txt");
            std::fs::write(&path, &r.text).map_err(|e| format!("{}: {e}", path.display()))?;
            for l in r.text.lines().skip(1).take(3) {
                println!("{l}");
            }
            println!("(per blob: {})", path.display());
            if r.bytes < min_bytes || r.rebuilds < min_rebuilds {
                return Err(format!(
                    "below the floor: bytes {} (at least {min_bytes}), rebuilds {} (at least {min_rebuilds})",
                    r.bytes, r.rebuilds
                ));
            }
            Ok(())
        }
        [cmd, kind, rom, out, pret, rest @ ..] if cmd == "check-pret" && kind == "sprites" => {
            let map = flag(rest, "--names")?.ok_or("check-pret sprites needs --names MAP")?;
            let min = floor(rest, "--min-strips")?;
            let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
            let sha = codegen::names::sha1_hex(&rom_bytes);
            let m = codegen::names::SourceMap::load(&sha, &[map])?;
            let names = m
                .data
                .into_iter()
                .map(|(a, i)| (a, (i.name, i.file)))
                .collect();
            let r = assets::sprites::check(Path::new(out), Path::new(pret), &names)?;
            let path = Path::new(out).join("pret-check-sprites.txt");
            std::fs::write(&path, &r.text).map_err(|e| format!("{}: {e}", path.display()))?;
            for l in r.text.lines().skip(1).take(2) {
                println!("{l}");
            }
            println!("(per sprite: {})", path.display());
            if r.strips.0 < min {
                return Err(format!(
                    "below the floor: {} strips at the image level (at least {min})",
                    r.strips.0
                ));
            }
            Ok(())
        }
        [cmd, rom, out, rest @ ..] if cmd == "graphics" => {
            export_graphics(rom, out, flag(rest, "--names")?, flag(rest, "--names-out")?)
        }
        [cmd, rom, out, rest @ ..] if cmd == "sprite-tables" => sprite_tables(rom, out, rest),
        [cmd, rom, dir, rest @ ..] if cmd == "check-load-log" => {
            let names = match rest {
                [] => None,
                [f, map] if f == "--names" => Some(map.as_str()),
                _ => return Err(USAGE.into()),
            };
            check_load_log(rom, dir, names)
        }
        [cmd, dir, session, rest @ ..] if cmd == "check-extract" => {
            let before = match flag(rest, "--before")? {
                Some(p) => Some(std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?),
                None => None,
            };
            let r = assets::export::check(Path::new(dir), Path::new(session), before.as_deref())?;
            let path = Path::new(session).join("extract-check.txt");
            std::fs::write(&path, &r.text).map_err(|e| format!("{}: {e}", path.display()))?;
            print!("{}", r.text.lines().next().unwrap_or(""));
            println!("\n(per blob: {})", path.display());
            let min_new = floor(rest, "--min-new")?;
            if r.missed > 0 || r.lost > 0 || r.new < min_new {
                return Err(format!(
                    "{} shown blobs missed, {} earlier entries lost or renamed, {} new (at least {min_new})",
                    r.missed, r.lost, r.new
                ));
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/// `gbaa-assets sprite-tables`.
fn sprite_tables(rom: &str, out: &str, rest: &[String]) -> Result<(), String> {
    use assets::spritetables as st;
    let bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
    let t = st::tables(&bytes, number(flag(rest, "--tables")?.ok_or(USAGE)?)?)?;
    let mut ids = Vec::new();
    if let Some(v) = flag(rest, "--anim-info")? {
        let (a, n) = v.split_once(':').ok_or("--anim-info ADDR:N")?;
        ids.extend(st::anim_info_ids(&bytes, number(a)?, number(n)?)?);
    }
    if let Some(v) = flag(rest, "--anims")? {
        for r in v.split(',') {
            let (a, b) = r.split_once('-').unwrap_or((r, r));
            ids.extend(number(a)?..=number(b)?);
        }
    }
    let labels = match flag(rest, "--labels")? {
        Some(p) => st::labels(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?)
            .map_err(|e| format!("{p}: {e}"))?,
        None => st::Labels::default(),
    };
    // A labelled animation is one the caller needs: always drawn.
    ids.extend(labels.anims.keys());
    ids.sort();
    ids.dedup();
    let fixed = flag(rest, "--palette")?.map(number).transpose()?;
    let dir = Path::new(out).join("sprite-tables");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let n = st::count(&bytes, &t);
    let (mut text, mut boxes) = (String::new(), String::new());
    boxes.push_str("anim\tvariant\tstep\tframe\tslot\tleft\ttop\tright\tbottom\tword\n");
    let (mut unread, mut boxed) = (Vec::new(), 0);
    for id in 0..n {
        match st::anim(&bytes, &t, id) {
            Ok(a) => {
                text.push_str(&st::describe(&a, &labels));
                let b = st::hitbox_lines(&a);
                boxed += usize::from(!b.is_empty());
                boxes.push_str(&b);
            }
            Err(e) => {
                let _ = writeln!(text, "anim {id}: {e}");
                unread.push(id);
            }
        }
    }
    let mut written = 0;
    for &id in &ids {
        let a = st::anim(&bytes, &t, id)?;
        if a.streamed {
            if let Some(l) = labels.anims.get(&id) {
                return Err(format!(
                    "anim {id} ({l}): streamed frames, which this can't draw"
                ));
            }
            println!("anim {id}: streamed frames, not drawn");
            continue;
        }
        let pal_at = match fixed {
            Some(p) => p,
            None => {
                let p = a
                    .variants
                    .iter()
                    .flat_map(|v| &v.steps)
                    .find_map(|s| s.palette);
                t.palette + 32 * p.map_or(0, |p| p.0 as u32)
            }
        };
        let pal = st::palette(&bytes, pal_at)?;
        let name = match labels.anims.get(&id) {
            Some(l) => format!("anim_{id:03}_{l}"),
            None => format!("anim_{id:03}"),
        };
        for (ext, data) in [
            ("aseprite", st::aseprite(&bytes, &t, &a, &pal, &labels)?),
            ("png", st::strip(&bytes, &t, &a, &pal)?),
        ] {
            let p = dir.join(format!("{name}.{ext}"));
            std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        written += 1;
    }
    for (f, s) in [("anims.txt", &text), ("hitboxes.tsv", &boxes)] {
        let p = dir.join(f);
        std::fs::write(&p, s).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    println!(
        "sprite tables {:#010x}: {n} animations ({} unread: {unread:?}), {boxed} with hitboxes; {written} drawn to {}",
        t.at,
        unread.len(),
        dir.display()
    );
    Ok(())
}

fn check_load_log(rom: &str, dir: &str, names: Option<&str>) -> Result<(), String> {
    let read = |name: &str| {
        let p = Path::new(dir).join(name);
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let mut syms = assets::loadlog::Symbols::new();
    if let Some(p) = names {
        let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
        let map = codegen::names::SourceMap::load(&codegen::names::sha1_hex(&rom_bytes), &[p])?;
        for (a, item) in map.data {
            syms.insert(a, (item.name, item.size));
        }
    }
    let r = assets::loadlog::check(
        &read("loads.tsv")?,
        &read("reached.tsv")?,
        &read("uses.tsv")?,
        &read("shown.tsv")?,
        &syms,
    )?;
    let path = Path::new(dir).join("check.txt");
    std::fs::write(&path, &r.text).map_err(|e| format!("{}: {e}", path.display()))?;
    for l in r.text.lines().filter(|l| !l.starts_with("  ")) {
        println!("{l}");
    }
    println!("(per blob: {})", path.display());
    if r.pass {
        Ok(())
    } else {
        Err("the load log misses something: see check.txt".into())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
