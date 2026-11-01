//! `gbaa-assets samples`: voicegroups, samples and soundfonts
//! ([`assets::samples`]) from the songs the manifest lists.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use assets::m4a::{self, Mem, ROM_BASE};
use assets::samples::{self, Bank, Group, Tone, Wave};
use assets::sf2;

/// The manifest's `[songs]`: header address to (first table index, name,
/// voicegroup).
fn manifest_songs(text: &str) -> Result<Vec<(usize, u32, String, u32)>, String> {
    let t: toml::Table = toml::from_str(text).map_err(|e| format!("manifest.toml: {e}"))?;
    let songs = t
        .get("songs")
        .and_then(|s| s.as_table())
        .ok_or("manifest has no [songs]: run `gbaa-assets songs` first")?;
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    let mut out = Vec::new();
    for (k, v) in songs {
        let index = v
            .get("index")
            .and_then(|i| i.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.as_integer())
            .unwrap_or(0) as usize;
        let name = v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let vg = v
            .get("voicegroup")
            .and_then(|n| n.as_integer())
            .unwrap_or(0) as u32;
        out.push((index, hex(k).ok_or(format!("[songs] key {k}"))?, name, vg));
    }
    out.sort();
    Ok(out)
}

pub fn export(rom_path: &str, out: &str, names: Option<&str>) -> Result<(), String> {
    let rom_bytes = std::fs::read(rom_path).map_err(|e| format!("{rom_path}: {e}"))?;
    let rom = Mem {
        base: ROM_BASE,
        bytes: &rom_bytes,
    };
    let sha = codegen::names::sha1_hex(&rom_bytes);
    let map = match names {
        Some(p) => Some(codegen::names::SourceMap::load(&sha, &[p])?),
        None => None,
    };
    let name_of = |a: u32, kind: &str| match map.as_ref().and_then(|m| m.data.get(&a)) {
        Some(item) => crate::file_name(&item.name),
        None => format!("{kind}_{a:08x}"),
    };
    let dir = Path::new(out);
    let manifest_path = dir.join("manifest.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let songs = manifest_songs(&manifest)?;

    // Each song's voicegroup, with the programs and keys it plays.
    let mut roots: BTreeMap<u32, Group> = BTreeMap::new();
    for (_, h, name, vg) in &songs {
        let song = m4a::decode_song(rom, *h)?;
        let g = roots.entry(*vg).or_default();
        for (p, keys) in samples::programs(&song) {
            g.keys.entry(p).or_default().extend(keys);
        }
        g.songs.insert(name.clone());
    }
    let mut groups = samples::walk(rom, &roots)?;
    let cry_tables = samples::cry_tables(rom, 16);
    for &(a, n) in &cry_tables {
        let g = groups.entry(a).or_default();
        g.table = true;
        for i in 0..n {
            if let Some(t) = Tone::read(rom, a + 12 * i as u32) {
                g.tones.insert(i.min(255) as u8, t);
            }
        }
    }
    // A cry table is longer than 256 entries: its tones by index.
    let mut cry_entries: BTreeMap<u32, Vec<Tone>> = BTreeMap::new();
    for &(a, n) in &cry_tables {
        cry_entries.insert(
            a,
            (0..n)
                .filter_map(|i| Tone::read(rom, a + 12 * i as u32))
                .collect(),
        );
    }

    // The waves every tone plays.
    let mut waves: BTreeMap<u32, Wave> = BTreeMap::new();
    let mut prog_waves: BTreeMap<u32, [u8; 16]> = BTreeMap::new();
    let mut bad = Vec::new();
    let all_tones = groups
        .values()
        .flat_map(|g| g.tones.values())
        .chain(cry_entries.values().flatten());
    for t in all_tones {
        if t.is_direct() {
            if let std::collections::btree_map::Entry::Vacant(e) = waves.entry(t.wav) {
                match Wave::read(rom, t.wav) {
                    Ok(w) => {
                        e.insert(w);
                    }
                    Err(err) => bad.push(err),
                }
            }
        } else if t.kind & 0xC7 == 3 {
            let mut b = [0u8; 16];
            for (i, v) in b.iter_mut().enumerate() {
                *v = rom.u8(t.wav + i as u32).unwrap_or(0);
            }
            prog_waves.insert(t.wav, b);
        }
    }

    // Samples no tone reached that sit right after one that is (pret's
    // unused ones), followed while each next one has a WaveData's shape.
    let reached = waves.len();
    let mut at: Vec<u32> = waves.values().map(Wave::next).collect();
    while let Some(a) = at.pop() {
        if waves.contains_key(&a) {
            continue;
        }
        let Ok(w) = Wave::read(rom, a) else { continue };
        if !(1024 * 1000..1024 * 100_000).contains(&w.pitch) || w.loop_start > w.size {
            continue;
        }
        at.push(w.next());
        waves.insert(a, w);
    }
    let neighbours = waves.len() - reached;

    let mut text = String::from("\n[samples]\n");
    std::fs::create_dir_all(dir.join("samples")).map_err(|e| format!("{out}: {e}"))?;
    let mut sample_file: BTreeMap<u32, String> = BTreeMap::new();
    for (a, w) in &waves {
        let file = format!("samples/{}.wav", name_of(*a, "sample"));
        std::fs::write(dir.join(&file), w.to_wav()).map_err(|e| format!("{file}: {e}"))?;
        let _ = writeln!(
            text,
            "{a:#010x} = {{ kind = \"{}\", pitch = {}, rate = {}, size = {}, looped = {}, loop_start = {}, file = \"{file}\" }}",
            if w.dpcm { "dpcm" } else { "pcm8" },
            w.pitch,
            (w.pitch + 512) / 1024,
            w.size,
            w.looped,
            w.loop_start
        );
        sample_file.insert(*a, file);
    }
    for (a, b) in &prog_waves {
        let file = format!("samples/{}.pcm", name_of(*a, "wave"));
        std::fs::write(dir.join(&file), b).map_err(|e| format!("{file}: {e}"))?;
        let _ = writeln!(
            text,
            "{a:#010x} = {{ kind = \"wave\", size = 16, file = \"{file}\" }}"
        );
        sample_file.insert(*a, file);
    }

    // Voicegroups as TOML.
    std::fs::create_dir_all(dir.join("voicegroups")).map_err(|e| format!("{out}: {e}"))?;
    std::fs::create_dir_all(dir.join("soundfonts")).map_err(|e| format!("{out}: {e}"))?;
    text += "\n[voicegroups]\n";
    for (a, g) in &groups {
        let name = name_of(*a, "voicegroup");
        let file = format!("voicegroups/{name}.toml");
        let mut t = format!(
            "# Written by `gbaa-assets samples` (crates/assets). Don't edit.\n\n[voicegroup]\nname = \"{name}\"\naddress = {a:#010x}\nsongs = {:?}\n",
            g.songs.iter().collect::<Vec<_>>()
        );
        let entries: Vec<(usize, Tone)> = match cry_entries.get(a) {
            Some(v) => v.iter().copied().enumerate().collect(),
            None => g.tones.iter().map(|(&p, &t)| (usize::from(p), t)).collect(),
        };
        for (p, tone) in &entries {
            let _ = write!(
                t,
                "\n[[voice]]\nprogram = {p}\ntype = {:#04x}\nkind = \"{}\"\nkey = {}\n",
                tone.kind,
                tone.kind_name(),
                tone.key
            );
            match tone.kind {
                0x40 | 0x80 => {
                    let _ = writeln!(t, "voicegroup = \"{}\"", name_of(tone.wav, "voicegroup"));
                    if tone.kind == 0x40 {
                        let keys: Vec<u8> = (0..=127)
                            .map(|k| rom.u8(tone.split_table() + k).unwrap_or(0))
                            .collect();
                        let _ = writeln!(
                            t,
                            "keysplit_table = {:#010x}\nkeysplit = {keys:?}",
                            tone.split_table()
                        );
                    }
                    continue;
                }
                _ => {}
            }
            let _ = writeln!(t, "pan = {}", tone.pan());
            if tone.kind & 0xC7 == 1 {
                let _ = writeln!(t, "sweep = {}", tone.pan_sweep);
            }
            match sample_file.get(&tone.wav) {
                Some(f) if tone.is_direct() || tone.kind & 7 == 3 => {
                    let _ = writeln!(t, "sample = \"../{f}\"");
                }
                _ if tone.kind & 7 == 4 => {
                    let _ = writeln!(t, "period = {}", tone.wav & 1);
                }
                _ if matches!(tone.kind & 7, 1 | 2) => {
                    let _ = writeln!(t, "duty = {}", tone.wav & 3);
                }
                _ => {
                    let _ = writeln!(t, "word = {:#010x}", tone.wav);
                }
            }
            let [at, de, su, re] = tone.adsr;
            let _ = writeln!(
                t,
                "attack = {at}\ndecay = {de}\nsustain = {su}\nrelease = {re}"
            );
        }
        std::fs::write(dir.join(&file), t).map_err(|e| format!("{file}: {e}"))?;

        // A soundfont for each song voicegroup.
        let mut sf = String::new();
        if roots.contains_key(a) {
            // Each file holds only its own samples.
            let mut own = Bank::default();
            let presets: Vec<sf2::Preset> = g
                .tones
                .iter()
                .map(|(&p, tone)| sf2::Preset {
                    name: format!("{p:03} {}", tone.kind_name()),
                    bank: 0,
                    program: u16::from(p),
                    zones: samples::program_zones(rom, &mut own, &waves, &groups, tone),
                })
                .filter(|p| !p.zones.is_empty())
                .collect();
            let path = format!("soundfonts/{name}.sf2");
            std::fs::write(dir.join(&path), sf2::write(&name, &own.samples, &presets))
                .map_err(|e| format!("{path}: {e}"))?;
            let _ = write!(sf, ", soundfont = \"{path}\"");
        }
        let _ = writeln!(
            text,
            "{a:#010x} = {{ name = \"{name}\", kind = \"{}\", voices = {}, songs = {}, file = \"{file}\"{sf} }}",
            if g.table {
                "cry_table"
            } else if roots.contains_key(a) {
                "song"
            } else {
                "nested"
            },
            entries.len(),
            g.songs.len()
        );
    }
    // The manifest: every other writer's sections as they were, these
    // two replaced.
    let text = assets::graphics::merge_manifest(
        &manifest,
        &["samples", "voicegroups"],
        text.trim_start_matches('\n'),
        "",
    );
    std::fs::write(&manifest_path, text)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    println!(
        "samples: {} waves ({} DPCM, {neighbours} found next to another), {} programmable waves; {} voicegroups ({} from songs, {} cry tables); {} soundfonts; {} unreadable",
        waves.len(),
        waves.values().filter(|w| w.dpcm).count(),
        prog_waves.len(),
        groups.len(),
        roots.len(),
        cry_tables.len(),
        roots.len(),
        bad.len()
    );
    for b in &bad {
        println!("  unreadable: {b}");
    }
    Ok(())
}
