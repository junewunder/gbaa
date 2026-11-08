use super::*;
use runtime::platform::{HEIGHT, WIDTH};
use std::time::Duration;

fn parse(args: &str) -> Result<Options, String> {
    Options::parse(args.split_whitespace().map(String::from))
}

#[test]
fn embedded_rom_beside_the_binary() {
    let dir = Path::new("/games/rogue");
    let rom = dir.join("emerald-rogue.gba");
    assert_eq!(
        embedded_path(&dir.join("gbaa-rogue.exe"), "emerald-rogue"),
        rom
    );
    assert_eq!(embedded_path(&dir.join("rogue"), "emerald-rogue"), rom);
}

/// `--timing`: the interpreter plays instant unless told, or unless a
/// recompiled build was generated otherwise (with `--interp` too).
#[test]
fn timing_defaults_to_instant_or_the_builds() {
    use runtime::Timing::{Exact, Instant, Static};
    static EXACT: Code = Code {
        blocks: &[],
        ram_blocks: &[],
        overlays: &[],
        extents: &[],
        ram_extents: &[],
        inner: &[],
        inner_extents: &[],
        modded: &[],
        folded: &[],
        rom_hash: 0,
        timing: Exact,
        seeds_file: None,
        spins: &[],
    };
    let plain = parse("game.gba").unwrap();
    assert_eq!(plain.timing(None), Instant);
    assert_eq!(plain.timing(Some(&EXACT)), Exact);
    assert_eq!(
        parse("game.gba --interp").unwrap().timing(Some(&EXACT)),
        Exact
    );
    let exact = parse("game.gba --timing exact").unwrap();
    assert_eq!(exact.timing(None), Exact);
    let stat = parse("game.gba --timing static").unwrap();
    assert_eq!(stat.timing(Some(&EXACT)), Static);
    assert!(parse("game.gba --timing none").is_err());
}

/// The game's folder: the highest layer set, else `saves/<name>/` in the
/// data dir; states follow the saves unless their own flag says.
#[test]
fn game_folder_layers() {
    use game_dir::{Folders, Layers, resolve};
    let data = Some(Path::new("/data/gbaa"));
    let p = |s: &str| PathBuf::from(s);
    let both = |d: &str| Folders {
        saves: p(d),
        states: p(d),
    };
    let mut l = Layers::default();
    assert_eq!(
        resolve(&l, "kirby", data),
        Some(both("/data/gbaa/saves/kirby"))
    );
    assert_eq!(resolve(&l, "kirby", None), None);
    // The global setting is a parent; the per-game one and the flag exact.
    l.global = Some(p("/sync"));
    assert_eq!(resolve(&l, "kirby", None), Some(both("/sync/kirby")));
    l.per_game = Some(p("/mine"));
    assert_eq!(resolve(&l, "kirby", data), Some(both("/mine")));
    l.save_flag = Some(p("/flag"));
    assert_eq!(resolve(&l, "kirby", data), Some(both("/flag")));
    l.state_flag = Some(p("/states"));
    let f = resolve(&l, "kirby", data).unwrap();
    assert_eq!(
        (f.saves.clone(), f.states.clone()),
        (p("/flag"), p("/states"))
    );
    // Files named after the game, even in its own folder.
    assert_eq!(f.sav("kirby"), p("/flag/kirby.sav"));
    assert_eq!(
        slots::path(&f.states_stem("kirby"), 2),
        p("/states/kirby.state2")
    );
    assert_eq!(
        resume::path(&f.states_stem("kirby")),
        p("/states/kirby.resume")
    );
    assert_eq!(
        resume::fault_path(&f.states_stem("kirby")),
        p("/states/kirby.resume.fault")
    );
}

#[test]
fn data_dir_per_os() {
    use std::ffi::OsString;
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    let home = env(&[
        ("HOME", "/home/me"),
        ("APPDATA", "C:\\Users\\me\\AppData\\Roaming"),
    ]);
    let xdg = env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/xdg")]);
    let got = game_dir::data_dir_in(home);
    if cfg!(windows) {
        assert_eq!(
            got,
            Some(Path::new("C:\\Users\\me\\AppData\\Roaming").join("gbaa"))
        );
    } else if cfg!(target_os = "macos") {
        assert_eq!(
            got,
            Some(PathBuf::from("/home/me/Library/Application Support/gbaa"))
        );
    } else {
        assert_eq!(got, Some(PathBuf::from("/home/me/.local/share/gbaa")));
        assert_eq!(game_dir::data_dir_in(xdg), Some(PathBuf::from("/xdg/gbaa")));
        assert_eq!(game_dir::data_dir_in(env(&[("HOME", "")])), None);
    }
}

/// Where the window keeps `game`'s save by default.
fn sav_in_folder(game: &str) -> PathBuf {
    game_dir::data_dir()
        .unwrap()
        .join("saves")
        .join(game)
        .join(format!("{game}.sav"))
}

#[test]
fn embedded_rom_unless_one_is_given() {
    static ROM: [u8; 4] = [1, 2, 3, 4];
    let exe = Path::new("/games/gbaa-rogue");
    let args = |a: &str| a.split_whitespace().map(String::from).collect::<Vec<_>>();
    let e = Embedded {
        rom: &ROM,
        name: "emerald-rogue",
    };
    let o = Options::parse_for(args("--headless"), Some(e), Some(exe)).unwrap();
    assert_eq!(o.rom, PathBuf::from("/games/emerald-rogue.gba"));
    assert_eq!(o.embedded, Some(e));
    assert_eq!(o.load_rom().unwrap(), &ROM);
    // The game's name is the one baked in, not the binary's.
    assert_eq!(o.game, "emerald-rogue");
    let o = Options::parse_for(args("other.gba --headless"), Some(e), Some(exe)).unwrap();
    assert_eq!(o.rom, PathBuf::from("other.gba"));
    assert_eq!(o.embedded, None);
    assert_eq!(o.game, "other");
    // The window's save goes in the game's folder, under its name.
    let o = Options::parse_for(args(""), Some(e), Some(exe)).unwrap();
    assert_eq!(o.save, Some(sav_in_folder("emerald-rogue")));
    assert_eq!(o.states, sav_in_folder("emerald-rogue").with_extension(""));
    assert!(Options::parse_for(args("--headless"), None, Some(exe)).is_err());
}

/// A ROM run directly is named from its file name, the rule a recompile
/// names its build by.
#[test]
fn game_named_from_the_rom_file() {
    let args = ["roms/Kirby & The Amazing Mirror.gba", "--headless"];
    let o = Options::parse(args.map(String::from)).unwrap();
    assert_eq!(o.game, "kirby-the-amazing-mirror");
    let o = parse("roms/!!!.gba --headless").unwrap();
    assert_eq!(o.game, "game");
}

#[test]
fn options() {
    let o = parse("game.gba --headless --dump out.png --hash").unwrap();
    assert_eq!(o.rom, PathBuf::from("game.gba"));
    assert!(o.headless && o.hash && !o.interp);
    assert_eq!(o.frames, Some(HEADLESS_FRAMES));
    assert_eq!(o.dump, Some(PathBuf::from("out.png")));
    let o = parse("--frames 10 --scale 4 game.gba --interp").unwrap();
    assert_eq!(
        (o.frames, o.scale, o.headless, o.interp),
        (Some(10), 4, false, true)
    );
    assert_eq!(parse("game.gba").unwrap().frames, None);
    assert_eq!(parse("game.gba").unwrap().state, None);
    let o = parse("game.gba --state at-30.ss").unwrap();
    assert_eq!(o.state, Some("at-30.ss".into()));
    assert!(parse("a.gba --state").is_err());
    assert!(parse("").is_err());
    assert!(parse("a.gba b.gba").is_err());
    assert!(parse("a.gba --frames").is_err());
    assert!(parse("a.gba --scale 0").is_err());
    assert!(parse("a.gba --frames 0").is_err());
    assert!(parse("a.gba --bogus").is_err());
}

#[test]
fn sound_options() {
    assert!(parse("a.gba --mute").unwrap().mute);
    // Headless never plays sound.
    assert!(parse("a.gba --headless --mute").is_err());
    let o = parse("a.gba --headless --audio-pace -0.5").unwrap();
    assert_eq!(o.audio_pace, Some(-0.005));
    assert!(parse("a.gba --audio-pace 0").is_err());
    assert!(parse("a.gba --headless --audio-pace fast").is_err());
}

#[test]
fn colours_widen_to_full_range() {
    assert_eq!(dump::rgb(0x7FFF), [255, 255, 255]);
    assert_eq!(dump::rgb(0x001F), [255, 0, 0]);
    assert_eq!(dump::rgb(0x03E0), [0, 255, 0]);
    assert_eq!(dump::rgb(0x7C00), [0, 0, 255]);
    assert_eq!(dump::rgb(0x0010), [132, 0, 0]);
}

#[test]
fn frames_become_sdl_xrgb() {
    let mut frame = Box::new([0u16; WIDTH * HEIGHT]);
    frame[0] = 0x001F;
    frame[WIDTH * HEIGHT - 1] = 0x7C00;
    let mut buf = vec![0xAA; WIDTH * HEIGHT * 4];
    crate::window::to_xrgb(&frame[..], &mut buf);
    let px = |i: usize| u32::from_ne_bytes(buf[i * 4..][..4].try_into().unwrap());
    assert_eq!(px(0), 0x00FF_0000);
    assert_eq!(px(1), 0);
    assert_eq!(px(WIDTH * HEIGHT - 1), 0x0000_00FF);
}

#[test]
fn hash_and_png() {
    let mut frame = Box::new([0u16; WIDTH * HEIGHT]);
    let blank = frame_hash(&frame);
    frame[100] = 1;
    assert_ne!(frame_hash(&frame), blank);
    let path = std::env::temp_dir().join(format!("gbaa-desktop-{}.png", std::process::id()));
    write_png(&path, &frame).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn input_scripts() {
    use runtime::io::key;
    let s = Script::parse("start@60, down@90+2,a@90").unwrap();
    assert_eq!(s.keys(59), 0);
    assert_eq!(s.keys(60), key::START);
    assert_eq!(s.keys(60 + netplay::script::HOLD - 1), key::START);
    assert_eq!(s.keys(60 + netplay::script::HOLD), 0);
    assert_eq!(s.keys(90), key::DOWN | key::A);
    assert_eq!(s.keys(91), key::DOWN | key::A);
    assert_eq!(s.keys(92), key::A);
    for bad in [
        "", "start", "start@", "@5", "jump@5", "a@x", "a@5+0", "a@5+", "a@-1",
    ] {
        assert!(Script::parse(bad).is_err(), "{bad:?}");
    }
    let o = parse("game.gba --headless --input select@3").unwrap();
    assert_eq!(o.input.keys(3), key::SELECT);
    assert!(parse("a.gba --input up@").is_err());
    assert_eq!(parse("a.gba").unwrap().input, Script::default());
}

#[test]
fn scripted_keys_follow_presented_frames() {
    use runtime::Platform;
    use runtime::io::key;
    let mut d = Desktop::headless().with_input(Script::parse("b@1+1").unwrap());
    let frame = Box::new([0u16; WIDTH * HEIGHT]);
    let keys: Vec<u16> = (0..3)
        .map(|_| {
            d.present(&frame);
            d.keys()
        })
        .collect();
    assert_eq!(keys, [0, key::B, 0]);
}

#[test]
fn audio_options_need_headless() {
    let o = parse("g.gba --headless --wav a.wav --fifo-dump a.fifo --audio-hash --audio-log a.log")
        .unwrap();
    assert!(o.audio() && o.audio_hash);
    assert_eq!(o.wav, Some(PathBuf::from("a.wav")));
    assert_eq!(o.fifo_dump, Some(PathBuf::from("a.fifo")));
    assert_eq!(o.audio_log, Some(PathBuf::from("a.log")));
    assert!(!parse("g.gba --headless").unwrap().audio());
    assert!(parse("g.gba --audio-hash").is_err());
}

#[test]
fn capture_dump_log_and_wav() {
    let mut c = Capture::default().with_samples();
    c.add(&runtime::AudioChunk {
        fifo: &[(1, -1), (2, -2)],
        samples: &[(256, -256), (512, -512)],
        period: 924,
        spans: &[(0, 924)],
        restarts: &[1],
        swaps: &[0],
        dropped: 0,
        psg: &[(16, -1)],
        psg_dropped: 0,
    });
    c.end_frame();
    c.add(&runtime::AudioChunk {
        fifo: &[(3, -3)],
        samples: &[(768, -768)],
        period: 924,
        spans: &[(0, 924)],
        restarts: &[0],
        swaps: &[],
        dropped: 2,
        psg: &[],
        psg_dropped: 1,
    });
    c.end_frame();
    assert_eq!(c.fifo, [1, 0xFF, 2, 0xFE, 3, 0xFD]);
    assert_eq!(c.psg, [16, 0, 0xFF, 0xFF]);
    assert_eq!(c.psg_dropped, 1);
    assert_eq!((c.records, c.dropped, c.rate()), (3, 2, 18157));
    assert_eq!(c.log(), "0 2 r1 s0\n1 1 r2\n");
    assert_eq!(c.hash(), capture::fnv1a(&c.fifo));
    let w = capture::wav(c.samples.as_deref().unwrap(), c.rate());
    assert_eq!(&w[..4], b"RIFF");
    assert_eq!(w.len(), 44 + 12);
    assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 18157);
    assert_eq!(&w[44..48], &[0x00, 0x01, 0x00, 0xFF]);
}

/// A launcher's --config-dir names the config folder, window or headless.
#[test]
fn config_dir_flag() {
    let o = parse("roms/Game.gba --config-dir /deck/gbaa-config").unwrap();
    assert_eq!(o.config_dir, Some(PathBuf::from("/deck/gbaa-config")));
    let o = parse("roms/Game.gba --headless --config-dir /c").unwrap();
    assert_eq!(o.config_dir, Some(PathBuf::from("/c")));
    assert_eq!(parse("roms/Game.gba").unwrap().config_dir, None);
    assert!(parse("roms/Game.gba --config-dir").is_err());
}

/// A launcher's --save-dir and --state-dir name the exact folders.
#[test]
fn save_dir_options() {
    let p = PathBuf::from;
    let o = parse("roms/Game.gba --save-dir /deck/saves/gba").unwrap();
    assert_eq!(o.save, Some(p("/deck/saves/gba/game.sav")));
    assert_eq!(o.states, p("/deck/saves/gba/game"));
    let o = parse("roms/Game.gba --save-dir /s --state-dir /t").unwrap();
    assert_eq!(o.save, Some(p("/s/game.sav")));
    assert_eq!(o.states, p("/t/game"));
    // Only the states moved: the .sav stays in the default folder.
    let o = parse("roms/Game.gba --state-dir /t").unwrap();
    assert_eq!(o.save, Some(sav_in_folder("game")));
    assert_eq!(o.states, p("/t/game"));
    // The default folder's files come in first, then those beside the ROM.
    let o = parse("roms/Game.gba --save-dir /s").unwrap();
    let default = sav_in_folder("game");
    assert_eq!(o.move_in.len(), 1);
    assert_eq!(o.move_in[0][0][0], (default.clone(), p("/s/game.sav")));
    assert_eq!(o.move_in[0][1][0], (p("roms/Game.sav"), p("/s/game.sav")));
    // With only --state-dir, the default folder's states on their own,
    // and its .sav (already in place) not among them.
    let o = parse("roms/Game.gba --state-dir /t").unwrap();
    assert_eq!(o.move_in.len(), 2);
    let states = &o.move_in[1][0];
    assert_eq!(
        states[0],
        (default.with_extension("state1"), p("/t/game.state1"))
    );
    assert!(states.iter().all(|(_, to)| to.starts_with("/t")));
    for args in [
        "a.gba --headless --save-dir /s",
        "a.gba --headless --state-dir /t",
        "a.gba --save-dir",
    ] {
        assert!(parse(args).is_err(), "{args}");
    }
}

/// The game's own setting and the global one place the window's files
/// when no flag does, and a folder the menu moved them from comes first.
#[test]
fn save_folder_settings_place_the_files() {
    use game_dir::GameConf;
    let p = PathBuf::from;
    let data = Some(Path::new("/data/gbaa"));
    let placed = |args: &str, placing: &Placing| {
        let mut o = parse(args).unwrap();
        o.place(placing, data);
        o
    };
    let global = Placing {
        global: Some(p("/sync")),
        ..Default::default()
    };
    let o = placed("roms/Game.gba", &global);
    assert_eq!(o.save, Some(p("/sync/game/game.sav")));
    assert_eq!(o.states, p("/sync/game/game"));
    // Its saves come in from the default folder, then from beside the ROM.
    assert_eq!(o.move_in.len(), 1);
    let from: Vec<_> = o.move_in[0].iter().map(|s| s[0].0.clone()).collect();
    assert_eq!(
        from,
        [p("/data/gbaa/saves/game/game.sav"), p("roms/Game.sav")]
    );
    // The game's own is exact, over the global; what it moved from and
    // the global it replaced come first.
    let mine = Placing {
        game: GameConf {
            save_dir: Some(p("/mine")),
            moved_from: Some(p("/old")),
        },
        global: Some(p("/sync")),
        previous_global: Some(p("/was")),
    };
    let o = placed("roms/Game.gba", &mine);
    assert_eq!(o.save, Some(p("/mine/game.sav")));
    assert_eq!(o.states, p("/mine/game"));
    let from: Vec<_> = o.move_in[0].iter().map(|s| s[0].0.clone()).collect();
    assert_eq!(
        from,
        [
            p("/old/game.sav"),
            p("/was/game/game.sav"),
            p("/data/gbaa/saves/game/game.sav"),
            p("roms/Game.sav")
        ]
    );
    // A flag is over both, and placing again keeps --save.
    let o = placed("roms/Game.gba --save-dir /flag", &mine);
    assert_eq!(o.save, Some(p("/flag/game.sav")));
    let o = placed("roms/Game.gba --save /x/my.sav", &mine);
    assert_eq!((o.save, o.move_in.len()), (Some(p("/x/my.sav")), 0));
    assert_eq!(o.states, p("/mine/game"));
    // Placing twice is placing once.
    let mut o = parse("roms/Game.gba").unwrap();
    o.place(&mine, data);
    o.place(&global, data);
    assert_eq!(o.save, Some(p("/sync/game/game.sav")));
    // The game's file.
    assert_eq!(
        GameConf::path(Path::new("/c"), "game"),
        p("/c/games/game.conf")
    );
    assert_eq!(
        GameConf::parse("save-dir = /a b\n# x\nmoved-from =\n"),
        Ok(GameConf {
            save_dir: Some(p("/a b")),
            moved_from: None
        })
    );
    assert!(GameConf::parse("colour = red").is_err());
    assert_eq!(GameConf::load(None), Ok(GameConf::default()));
}

/// With --save-dir, the saves played in the default folder come in, and
/// those beside the ROM only when it has none.
#[test]
fn save_dir_moves_in_from_default() {
    let dir = std::env::temp_dir().join(format!("gbaa-save-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let folders = |d: &str| game_dir::Folders {
        saves: dir.join(d),
        states: dir.join(d),
    };
    let (default, flag) = (folders("default"), folders("flag"));
    std::fs::create_dir_all(&default.saves).unwrap();
    std::fs::create_dir_all(&flag.saves).unwrap();
    let rom = dir.join("Game.gba");
    std::fs::write(rom.with_extension("sav"), b"old").unwrap();
    std::fs::write(rom.with_extension("state3"), b"old").unwrap();
    std::fs::write(default.sav("game"), b"played").unwrap();
    std::fs::write(resume::path(&default.states_stem("game")), b"resume").unwrap();
    let sources = vec![
        game_dir::folder_pairs(&default, &flag, "game"),
        game_dir::move_in_pairs(&rom, &flag.sav("game"), &flag.states_stem("game")),
    ];
    let copied = game_dir::move_in(&sources).unwrap();
    assert_eq!(
        copied,
        [flag.sav("game"), resume::path(&flag.states_stem("game"))]
    );
    assert_eq!(std::fs::read(flag.sav("game")).unwrap(), b"played");
    // Nothing mixed in from beside the ROM, and nothing on a second run.
    assert!(!slots::path(&flag.states_stem("game"), 3).exists());
    std::fs::remove_file(flag.sav("game")).unwrap();
    assert_eq!(game_dir::move_in(&sources).unwrap(), Vec::<PathBuf>::new());
    // A fresh flag folder with nothing in the default one: beside the ROM.
    let fresh = folders("fresh");
    std::fs::create_dir_all(&fresh.saves).unwrap();
    let empty = folders("empty");
    let sources = vec![
        game_dir::folder_pairs(&empty, &fresh, "game"),
        game_dir::move_in_pairs(&rom, &fresh.sav("game"), &fresh.states_stem("game")),
    ];
    assert_eq!(
        game_dir::move_in(&sources).unwrap(),
        [
            fresh.sav("game"),
            slots::path(&fresh.states_stem("game"), 3)
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_options() {
    // The window keeps NAME.sav in the game's folder.
    let o = parse("roms/Game.gba").unwrap();
    assert_eq!(o.save, Some(sav_in_folder("game")));
    assert_eq!(parse("a.gba --no-save").unwrap().save, None);
    assert_eq!(parse("a.gba --headless").unwrap().save, None);
    // Headless, a .sav only as given, and a script's slots beside the ROM.
    let o = parse("roms/a.gba --headless --save x.sav").unwrap();
    assert_eq!(o.save, Some(PathBuf::from("x.sav")));
    assert_eq!(o.states, PathBuf::from("roms/a.gba"));
    assert!(parse("a.gba --headless --no-save").is_err());
    assert!(parse("a.gba --save x.sav --no-save").is_err());
}

/// The window's files beside the ROM move into the game's folder on its
/// first run there: the .sav only when the folder keeps it.
#[test]
fn move_in_options() {
    let o = parse("roms/Game.gba").unwrap();
    let stem = sav_in_folder("game").with_extension("");
    assert_eq!(o.move_in.len(), 1);
    assert_eq!(o.move_in[0].len(), 1);
    let beside = &o.move_in[0][0];
    let first = &beside[..2];
    assert_eq!(
        first,
        [
            (PathBuf::from("roms/Game.sav"), sav_in_folder("game")),
            (
                PathBuf::from("roms/Game.state1"),
                stem.with_extension("state1")
            ),
        ]
    );
    assert_eq!(
        beside.last(),
        Some(&(
            PathBuf::from("roms/Game.resume.old"),
            stem.with_extension("resume.old")
        ))
    );
    // With --save or --no-save nothing moves in, so a later normal run
    // still finds the folder empty and brings the .sav with the rest.
    for args in [
        "roms/Game.gba --save x.sav",
        "roms/Game.gba --no-save",
        "roms/Game.gba --headless",
    ] {
        assert!(parse(args).unwrap().move_in.is_empty(), "{args}");
    }
}

/// Copied in, renamed, originals kept; never over a file already there.
#[test]
fn move_in_copies_once() {
    let dir = std::env::temp_dir().join(format!("gbaa-move-in-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let folder = dir.join("saves/game");
    std::fs::create_dir_all(&folder).unwrap();
    let rom = dir.join("Game.gba");
    std::fs::write(rom.with_extension("sav"), b"save").unwrap();
    std::fs::write(rom.with_extension("state2"), b"state").unwrap();
    std::fs::write(rom.with_extension("resume"), b"resume").unwrap();
    // A --no-save run first copies nothing, so the next run brings the .sav.
    let no_save = parse(&format!("{} --no-save", rom.display())).unwrap();
    assert_eq!(
        no_save
            .move_in
            .iter()
            .map(|m| game_dir::move_in(m).unwrap())
            .collect::<Vec<_>>(),
        Vec::<Vec<PathBuf>>::new()
    );
    assert_eq!(game_dir::move_in(&[]).unwrap(), Vec::<PathBuf>::new());
    let stem = folder.join("game");
    let pairs = vec![game_dir::move_in_pairs(
        &rom,
        &stem.with_extension("sav"),
        &stem,
    )];
    let copied = game_dir::move_in(&pairs).unwrap();
    assert_eq!(
        copied,
        ["sav", "state2", "resume"].map(|e| stem.with_extension(e))
    );
    assert_eq!(std::fs::read(stem.with_extension("sav")).unwrap(), b"save");
    assert_eq!(
        std::fs::read(stem.with_extension("resume")).unwrap(),
        b"resume"
    );
    assert!(rom.with_extension("sav").exists());
    // Once the folder has any of the game's files, nothing more comes in.
    std::fs::write(rom.with_extension("sav"), b"newer").unwrap();
    std::fs::remove_file(stem.with_extension("sav")).unwrap();
    std::fs::write(rom.with_extension("state1"), b"other").unwrap();
    assert_eq!(game_dir::move_in(&pairs).unwrap(), Vec::<PathBuf>::new());
    assert!(!stem.with_extension("sav").exists());
    assert!(!stem.with_extension("state1").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sav_file_round_trips_and_keeps_the_rtc_record() {
    use runtime::save::{Kind, SAVE_SIZE, Save};
    let dir = std::env::temp_dir().join(format!("gbaa-sav-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("game.sav");
    // mGBA's file: 128 KB of flash, then a 16-byte RTC record.
    let mut bytes = vec![0x11; SAVE_SIZE];
    bytes.extend_from_slice(&[0xAB; 16]);
    std::fs::write(&path, &bytes).unwrap();

    let mut s = Box::new(Save::new(Kind::Flash1M));
    let mut f = sav_file::SavFile::open(&path, &mut s).unwrap();
    assert_eq!(s.data[SAVE_SIZE - 1], 0x11);
    // A program: the file waits for the save to settle.
    s.write(0x5555, 0xAA);
    s.write(0x2AAA, 0x55);
    s.write(0x5555, 0xA0);
    s.write(0x0010, 0x01);
    for _ in 0..sav_file::SETTLE_FRAMES {
        f.frame(&mut s).unwrap();
        assert_eq!(std::fs::read(&path).unwrap()[0x10], 0x11);
    }
    f.frame(&mut s).unwrap();
    let written = std::fs::read(&path).unwrap();
    assert_eq!(written.len(), SAVE_SIZE + 16);
    assert_eq!(written[0x10], 0x01);
    assert_eq!(&written[SAVE_SIZE..], &[0xAB; 16]);

    // SRAM's file is mGBA's 32 KB; a missing file starts blank and is
    // written on exit.
    let path = dir.join("sram.sav");
    let _ = std::fs::remove_file(&path);
    let mut s = Box::new(Save::new(Kind::Sram));
    let mut f = sav_file::SavFile::open(&path, &mut s).unwrap();
    s.write(0x7FFF, 0x5A);
    f.flush(&mut s).unwrap();
    let written = std::fs::read(&path).unwrap();
    assert_eq!((written.len(), written[0x7FFF]), (0x8000, 0x5A));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sav_backups_on_exit() {
    let dir = std::env::temp_dir().join(format!("gbaa-sav-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sav = dir.join("Emerald.sav");
    let saves = sav_file::backup_dir(&sav);
    assert_eq!(saves, dir.join("backups"));
    // No .sav yet: nothing to back up.
    assert_eq!(sav_file::backup(&sav, "20261024-101500").unwrap(), None);
    assert!(!saves.exists());
    std::fs::write(&sav, b"one").unwrap();
    let first = saves.join("Emerald-20261024-101500.sav");
    assert_eq!(
        sav_file::backup(&sav, "20261024-101500").unwrap(),
        Some(first.clone())
    );
    assert_eq!(std::fs::read(&first).unwrap(), b"one");
    // Unchanged since the newest backup: skipped. Another game's backups
    // in the same folder don't count.
    std::fs::write(saves.join("Emerald-hack-20991231-235959.sav"), b"two").unwrap();
    assert_eq!(sav_file::backup(&sav, "20261024-111500").unwrap(), None);
    std::fs::write(&sav, b"two").unwrap();
    let second = saves.join("Emerald-20261024-111500.sav");
    assert_eq!(
        sav_file::backup(&sav, "20261024-111500").unwrap(),
        Some(second.clone())
    );
    // Only the newest is compared: going back to an older save is backed up.
    std::fs::write(&sav, b"one").unwrap();
    assert!(sav_file::backup(&sav, "20261024-121500").unwrap().is_some());
    assert_eq!(std::fs::read(&first).unwrap(), b"one");
    assert_eq!(std::fs::read(&second).unwrap(), b"two");
    let stamp = sav_file::stamp_now();
    assert!(stamp.len() == 15 && stamp.as_bytes()[8] == b'-', "{stamp}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sav_backups_pruned_past_cap() {
    let dir = std::env::temp_dir().join(format!("gbaa-sav-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let kb = |n: usize| vec![0u8; n * 1024];
    // 9 KB of backups over three games; Ruby has only one.
    let files = [
        ("Emerald-20261020-100000.sav", 1),
        ("Emerald-hack-20261020-110000.sav", 1),
        ("Ruby-20261020-120000.sav", 1),
        ("Emerald-20261021-100000.sav", 1),
        ("Emerald-hack-20261022-100000.sav", 1),
        ("Emerald-20261023-100000.sav", 1),
        ("Emerald-hack-20261024-100000.sav", 3),
    ];
    for (f, n) in files {
        std::fs::write(dir.join(f), kb(n)).unwrap();
    }
    // Not a backup: neither counted nor deleted.
    std::fs::write(dir.join("notes.txt"), kb(50)).unwrap();
    // Under the cap: nothing goes.
    assert!(sav_file::prune(&dir, 9 * 1024).unwrap().is_empty());
    // Down to 5 KB: the oldest go, Ruby's only backup stays though older.
    let gone = sav_file::prune(&dir, 5 * 1024).unwrap();
    let names: Vec<_> = gone
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "Emerald-20261020-100000.sav",
            "Emerald-hack-20261020-110000.sav",
            "Emerald-20261021-100000.sav",
            "Emerald-hack-20261022-100000.sav",
        ]
    );
    // Past what can go: each game keeps its newest, over the cap.
    assert!(sav_file::prune(&dir, 0).unwrap().is_empty());
    let mut left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "Emerald-20261023-100000.sav",
            "Emerald-hack-20261024-100000.sav",
            "Ruby-20261020-120000.sav",
            "notes.txt",
        ]
    );
    // No folder: nothing to do.
    assert!(sav_file::prune(&dir.join("none"), 0).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn gamepad_mappings() {
    use gamepad::Mapping;
    use runtime::io::key;
    use sdl3::gamepad::Button;
    let d = Mapping::default();
    assert_eq!(d, Mapping::parse(gamepad::DEFAULT).unwrap());
    // By default the buttons across from A and B press them too, and no
    // face button saves or loads a state.
    let mut pads = gamepad::Pads::new(d.clone());
    let mut press = |b| {
        pads.feed(0, gamepad::PadEvent::Pressed(b));
        let k = pads.keys();
        pads.feed(0, gamepad::PadEvent::Released(b));
        k
    };
    assert_eq!(press(Button::South), key::A);
    assert_eq!(press(Button::North), key::A);
    assert_eq!(press(Button::East), key::B);
    assert_eq!(press(Button::West), key::B);
    // A file replaces only the buttons it names; an empty list unbinds.
    let m = d
        .with("# positions\na = east west  # two\nb = south\nselect =\n")
        .unwrap();
    let mut pads = gamepad::Pads::new(m);
    let mut press = |b| {
        pads.feed(0, gamepad::PadEvent::Pressed(b));
        let k = pads.keys();
        pads.feed(0, gamepad::PadEvent::Released(b));
        k
    };
    assert_eq!(press(Button::East), key::A);
    // West's default (B, by its label X) went with b's line.
    assert_eq!(press(Button::West), key::A);
    assert_eq!(press(Button::South), key::B);
    assert_eq!(press(Button::Back), 0);
    assert_eq!(press(Button::Start), key::START);
    // Triggers are axes in SDL: pulled past halfway, they're down.
    pads.feed(
        0,
        gamepad::PadEvent::Axis(sdl3::gamepad::Axis::TriggerLeft, 0.8),
    );
    assert_eq!(pads.keys(), key::L);
    for bad in [
        "x = south",
        "a = shout",
        "a south",
        "[BPEE]",
        "a = east\na = west",
    ] {
        assert!(d.with(bad).is_err(), "{bad:?}");
    }
    assert!(parse("a.gba --headless --pad-config p.conf").is_err());
    assert_eq!(
        parse("a.gba --pad-config p.conf").unwrap().pad_config,
        Some("p.conf".into())
    );
}

#[test]
fn gamepads_and_hotplug() {
    use gamepad::PadEvent::*;
    use runtime::io::key;
    use sdl3::gamepad::{Axis, Button};
    let mut pads = gamepad::Pads::new(gamepad::Mapping::default());
    pads.feed(0, Pressed(Button::South));
    pads.feed(1, Pressed(Button::RightShoulder));
    pads.feed(1, Axis(Axis::LeftY, 0.9));
    assert_eq!(pads.keys(), key::A | key::R | key::DOWN);
    // Inside the dead zone, or the other way, isn't down.
    pads.feed(1, Axis(Axis::LeftY, 0.1));
    assert_eq!(pads.keys(), key::A | key::R);
    // The sticks' is 40% by default: a stray third of the way is
    // nothing, the triggers' still 20%.
    pads.feed(1, Axis(Axis::LeftY, -0.35));
    assert_eq!(pads.keys(), key::A | key::R);
    pads.feed(1, Axis(Axis::TriggerLeft, 0.3));
    assert_eq!(pads.keys(), key::A | key::R | key::L);
    pads.feed(1, Axis(Axis::TriggerLeft, 0.0));
    pads.feed(1, Axis(Axis::LeftY, 0.0));
    pads.feed(1, Axis(Axis::LeftX, 0.7));
    assert_eq!(pads.keys(), key::A | key::R | key::RIGHT);
    // Wider deadzones (the settings file's) let a part push go: the
    // sticks' and the triggers' each their own.
    pads.deadzones = gamepad::Deadzones::percent(80, 20);
    assert_eq!(pads.keys(), key::A | key::R);
    pads.feed(1, Axis(Axis::LeftX, 0.0));
    pads.feed(1, Axis(Axis::TriggerLeft, 0.3));
    assert_eq!(pads.keys(), key::A | key::R | key::L);
    pads.deadzones = gamepad::Deadzones::percent(80, 40);
    assert_eq!(pads.keys(), key::A | key::R);
    pads.feed(1, Axis(Axis::TriggerLeft, 0.0));
    // Apart, up and down take their own: a wide one there lets a stray
    // up go while the same push sideways still counts, on either stick.
    pads.deadzones = gamepad::Deadzones::percent_axes(30, 70, 20);
    pads.feed(1, Axis(Axis::LeftX, 0.5));
    assert_eq!(pads.keys(), key::A | key::R | key::RIGHT);
    pads.feed(1, Axis(Axis::LeftX, 0.0));
    pads.feed(1, Axis(Axis::LeftY, -0.5));
    assert_eq!(pads.keys(), key::A | key::R);
    pads.feed(1, Axis(Axis::LeftY, -0.75));
    assert_eq!(pads.keys(), key::A | key::R | key::UP);
    pads.feed(1, Axis(Axis::LeftY, 0.0));
    pads.feed(1, Axis(Axis::LeftX, -0.35));
    assert_eq!(pads.keys(), key::A | key::R | key::LEFT);
    pads.feed(1, Axis(Axis::LeftX, 0.0));
    assert_eq!(pads.deadzones.of(Axis::RightY), 0.7);
    assert_eq!(pads.deadzones.of(Axis::RightX), 0.3);
    // Together, both ways are the one.
    assert_eq!(
        gamepad::Deadzones::percent(55, 20),
        gamepad::Deadzones::percent_axes(55, 55, 20)
    );
    pads.deadzones = gamepad::Deadzones::default();
    pads.feed(1, Axis(Axis::LeftX, 0.7));
    // Two pads holding one button: it stays down until both let go.
    pads.feed(1, Pressed(Button::South));
    pads.feed(0, Released(Button::South));
    assert_eq!(pads.keys(), key::A | key::R | key::RIGHT);
    // Unplugged mid-press: all it held goes up; plugged back, it starts clean.
    pads.feed(1, Gone);
    assert_eq!(pads.keys(), 0);
    pads.feed(1, Pressed(Button::Start));
    assert_eq!(pads.keys(), key::START);
}

#[test]
fn face_buttons_by_label() {
    use gamepad::PadEvent::*;
    use runtime::io::key;
    use sdl3::gamepad::Button;
    let m = gamepad::Mapping::default()
        .with("a = label-a\nb = label-b\n")
        .unwrap();
    let mut pads = gamepad::Pads::new(m);
    let mut press = |pad, b| {
        pads.feed(pad, Pressed(b));
        let k = pads.keys();
        pads.feed(pad, Released(b));
        k
    };
    // Until a pad says otherwise, its letters are where Xbox's are.
    assert_eq!(press(0, Button::South), key::A);
    assert_eq!(press(0, Button::East), key::B);
    // A Nintendo layout: A on the east, B on the south.
    let nintendo = [Button::East, Button::South, Button::North, Button::West];
    pads.feed(1, Labels(nintendo));
    let mut press = |pad, b| {
        pads.feed(pad, Pressed(b));
        let k = pads.keys();
        pads.feed(pad, Released(b));
        k
    };
    assert_eq!(press(1, Button::East), key::A);
    assert_eq!(press(1, Button::South), key::B);
    assert_eq!(press(0, Button::South), key::A);
    assert!(gamepad::Mapping::default().with("a = label-z").is_err());
}

#[test]
fn gamepad_events_reach_keyinput() {
    use gamepad::PadEvent::*;
    use runtime::io::{key, reg};
    use sdl3::gamepad::Button;
    // `b .`, so frames run with nothing else happening.
    let rom: &'static [u8] = &[0xFE, 0xFF, 0xFF, 0xEA];
    let pads = gamepad::Pads::new(gamepad::Mapping::default());
    let mut m = Box::new(runtime::Machine::new(
        rom,
        Desktop::headless().with_pads(pads),
    ));
    let keyinput = |m: &DesktopMachine| m.bus.read16(0x0400_0000 + reg::KEYINPUT);
    m.run_frame();
    assert_eq!(keyinput(&m), key::ALL);
    let step = |m: &mut DesktopMachine, e| {
        m.platform.pads().unwrap().feed(3, e);
        m.run_frame();
        !keyinput(m) & key::ALL
    };
    assert_eq!(step(&mut m, Pressed(Button::South)), key::A);
    assert_eq!(step(&mut m, Pressed(Button::DPadUp)), key::A | key::UP);
    assert_eq!(step(&mut m, Released(Button::South)), key::UP);
    assert_eq!(step(&mut m, Gone), 0);
}

#[test]
fn fast_forward_options() {
    let o = parse("a.gba --fast-speed 4 --fast-forward").unwrap();
    assert_eq!((o.fast_speed, o.fast_forward), (Some(4.0), true));
    assert_eq!(parse("a.gba").unwrap().fast_speed, None);
    let speed = |a: &str| parse(&format!("a.gba --fast-speed {a}")).map(|o| o.fast_speed);
    assert_eq!(speed("2.5"), Ok(Some(2.5)));
    assert_eq!(speed("1.5"), Ok(Some(1.5)));
    assert_eq!(speed("unlimited"), Ok(Some(0.0)));
    assert_eq!(speed("0"), Ok(Some(0.0)));
    for bad in ["1", "17", "-2", "fast", "NaN"] {
        assert!(speed(bad).is_err(), "{bad}");
    }
    assert!(parse("a.gba --headless --fast-forward").is_err());
    assert!(parse("a.gba --headless --fast-speed 2").is_err());
}

#[test]
fn fast_forward_holds_and_toggles() {
    let mut f = fast::FastForward::new(0.0, false);
    f.input(true, false);
    assert!(f.active());
    f.input(false, false);
    assert!(!f.active());
    // The toggle flips on its press, not while it's held.
    f.input(false, true);
    f.input(false, true);
    assert!(f.toggled);
    f.input(false, false);
    f.input(false, true);
    assert!(!f.active());
    assert_eq!(f.period(), None);
    assert_eq!(
        fast::FastForward::new(4.0, false).period(),
        Some(fast::frame() / 4)
    );
    let p = fast::FastForward::new(2.5, false).period().unwrap();
    assert!(p.abs_diff(fast::frame() * 2 / 5) < std::time::Duration::from_micros(1));
}

/// The frames fast-forward draws, at `period` apart for `secs` seconds on a
/// made-up clock: the gaps between draws, in ms.
fn draw_gaps(period: Duration, secs: u32) -> Vec<f64> {
    let start = std::time::Instant::now();
    let mut pacer = fast::DrawPacer::default();
    let mut drawn = vec![];
    let mut t = Duration::ZERO;
    while t < Duration::from_secs(secs.into()) {
        if pacer.due(start + t) {
            drawn.push(t);
        }
        t += period;
    }
    drawn
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0)
        .collect()
}

#[test]
fn fast_forward_draws_sixty_a_second() {
    let tick = 1000.0 / fast::DRAW_HZ;
    for speed in [1.5, 2.5, 4.0, 16.0] {
        let period = fast::FastForward::new(speed, true).period().unwrap();
        let gaps = draw_gaps(period, 10);
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        // 60 a second on average, not 50 (2.5x counted from the last draw).
        assert!((mean - tick).abs() < 0.05, "{speed}x: mean {mean:.2} ms");
        // Each the first frame past its deadline: never a frame further.
        let frame = period.as_secs_f64() * 1000.0;
        assert!(
            gaps.iter().all(|g| (g - tick).abs() < frame + 0.01),
            "{speed}x: {gaps:?}"
        );
    }
    // Frames slower than the schedule: every one drawn, and no burst to
    // catch up after a stall.
    let gaps = draw_gaps(Duration::from_millis(25), 2);
    assert!(gaps.iter().all(|g| (g - 25.0).abs() < 0.01), "{gaps:?}");
    let start = std::time::Instant::now();
    let mut pacer = fast::DrawPacer::default();
    assert!(pacer.due(start));
    assert!(pacer.due(start + Duration::from_millis(100)));
    assert!(!pacer.due(start + Duration::from_millis(105)));
}

#[test]
fn the_settings_file() {
    use settings::{Settings, load, parse, set};
    assert_eq!(
        parse(
            "# speed\n fast-speed = 1.5 # slower\nvolume = 60%\nserver = 10.0.0.2:7374\n\
             directions = apart\ndeadzone = 25%\ntrigger-deadzone = 10\nresume = off\n\
             deadzone-axes = apart\ndeadzone-horizontal = 30\ndeadzone-vertical = 60%\n"
        ),
        Ok(Settings {
            fast_speed: Some(1.5),
            volume: Some(60),
            server: Some("10.0.0.2:7374".into()),
            directions_apart: Some(true),
            deadzone: Some(25),
            trigger_deadzone: Some(10),
            deadzone_axes_apart: Some(true),
            deadzone_horizontal: Some(30),
            deadzone_vertical: Some(60),
            resume: Some(false),
            save_dir: None,
            previous_save_dir: None,
        })
    );
    // All saves in one folder; empty is the default.
    let s = parse("save-dir = /sync/gba\nprevious-save-dir =\n").unwrap();
    assert_eq!(s.save_dir, Some(PathBuf::from("/sync/gba")));
    assert_eq!(s.previous_save_dir, None);
    assert_eq!(parse("save-dir =").unwrap().save_dir, None);
    assert_eq!(parse("fast-speed=unlimited").unwrap().fast_speed, Some(0.0));
    assert_eq!(parse("\n# nothing\n"), Ok(Settings::default()));
    assert!(parse("fast-speed = 20").is_err());
    assert!(parse("volume = 201").is_err());
    assert!(parse("volume = -1").is_err());
    assert!(parse("speed = 2").is_err());
    assert!(parse("fast-speed 2").is_err());
    assert!(parse("directions = sideways").is_err());
    assert!(parse("resume = maybe").is_err());
    assert_eq!(parse("resume = on").unwrap().resume, Some(true));
    // Resume on launch is off unless the file turns it on.
    assert!(!parse("").unwrap().resume_on());
    assert!(!parse("resume = off").unwrap().resume_on());
    assert!(parse("resume = on").unwrap().resume_on());
    assert!(parse("deadzone = 95").is_err());
    assert!(parse("deadzone = lots").is_err());
    assert!(parse("trigger-deadzone = 4").is_err());
    assert!(parse("deadzone-axes = crossed").is_err());
    assert!(parse("deadzone-vertical = 91").is_err());
    assert_eq!(
        parse("deadzone-axes = together")
            .unwrap()
            .deadzone_axes_apart,
        Some(false)
    );
    assert_eq!(
        parse("trigger-deadzone = 5").unwrap().trigger_deadzone,
        Some(5)
    );
    assert_eq!(
        parse("directions = together").unwrap().directions_apart,
        Some(false)
    );
    // set rewrites the key's line, comments kept, or adds one.
    assert_eq!(
        set(
            "# mine\nvolume = 50 # quiet\nfast-speed = 2\nvolume=70\n",
            "volume",
            "80"
        ),
        "# mine\nvolume = 80 # quiet\nfast-speed = 2\n"
    );
    assert_eq!(
        set("fast-speed = 2", "volume", "80"),
        "fast-speed = 2\nvolume = 80\n"
    );
    assert_eq!(set("", "volume", "0"), "volume = 0\n");
    let dir = std::env::temp_dir().join(format!("gbaa-settings-{}", std::process::id()));
    let path = dir.join("settings.conf");
    // No file: nothing set. save makes the folder and file; load reads it.
    assert_eq!(load(Some(&path)), Ok(Settings::default()));
    assert_eq!(load(None), Ok(Settings::default()));
    settings::save(&path, "volume", "150").unwrap();
    settings::save(&path, "fast-speed", "3.5").unwrap();
    assert_eq!(
        load(Some(&path)),
        Ok(Settings {
            fast_speed: Some(3.5),
            volume: Some(150),
            server: None,
            directions_apart: None,
            deadzone: None,
            trigger_deadzone: None,
            deadzone_axes_apart: None,
            deadzone_horizontal: None,
            deadzone_vertical: None,
            resume: None,
            save_dir: None,
            previous_save_dir: None,
        })
    );
    std::fs::write(&path, "fast-speed = 99\n").unwrap();
    let e = load(Some(&path)).unwrap_err();
    assert!(e.contains("settings.conf: line 1"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_volume_flag() {
    use audio::{DEFAULT_GAIN, volume_gain};
    let volume = |a: &str| parse(&format!("a.gba {a}")).map(|o| (o.volume, o.gain()));
    assert_eq!(volume(""), Ok((None, DEFAULT_GAIN)));
    assert_eq!(volume("--volume 100"), Ok((Some(100), DEFAULT_GAIN)));
    assert_eq!(volume("--volume 200%"), Ok((Some(200), 1.0)));
    assert_eq!(volume("--volume 0"), Ok((Some(0), 0.0)));
    assert_eq!(volume_gain(50), DEFAULT_GAIN / 2.0);
    assert!(volume("--volume 250").is_err());
    assert!(volume("--volume loud").is_err());
    // Headless plays nothing, so only --audio-pace's device takes it.
    assert!(volume("--headless --volume 50").is_err());
    assert!(volume("--headless --frames 10 --audio-pace 0 --volume 50").is_ok());
}

#[test]
fn gbaa_config_dir_moves_the_config_files() {
    use gamepad::{Control, config_dir_in};
    use sdl3::gamepad::Button;
    use std::ffi::OsString;
    let tmp = std::env::temp_dir().join(format!("gbaa-config-dir-{}", std::process::id()));
    let (lane, home, xdg) = (tmp.join("lane"), tmp.join("home"), tmp.join("xdg"));
    let env = |vars: &[(&str, &Path)]| {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_os_str().to_owned()))
            .collect();
        move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    };
    // Without it: XDG_CONFIG_HOME's, else HOME's .config; with it, it wins
    // over both (an empty one counts as unset).
    if !cfg!(windows) {
        let home_only = env(&[("HOME", &home)]);
        assert_eq!(config_dir_in(home_only), Some(home.join(".config/gbaa")));
        let both = env(&[("HOME", &home), ("XDG_CONFIG_HOME", &xdg)]);
        assert_eq!(config_dir_in(both), Some(xdg.join("gbaa")));
        let empty = env(&[("HOME", &home), ("GBAA_CONFIG_DIR", Path::new(""))]);
        assert_eq!(config_dir_in(empty), Some(home.join(".config/gbaa")));
    }
    let set = env(&[
        ("HOME", &home),
        ("XDG_CONFIG_HOME", &xdg),
        ("APPDATA", &xdg),
        ("GBAA_CONFIG_DIR", &lane),
    ]);
    let dir = config_dir_in(set).unwrap();
    assert_eq!(dir, lane);
    // The remap screen's save writes there, and the next run reads it back.
    let rom = remap::RomId {
        hash: 1,
        title: "GAME".into(),
    };
    let path = dir.join("gamepad.conf");
    let mut c = remap::Controls::load(&path, false, rom.clone()).unwrap();
    let east = Control::Button(Button::East);
    c.add(false, runtime::io::key::A, east);
    assert_eq!(c.save(), Ok(Some(path.clone())));
    let again = remap::Controls::load(&path, false, rom).unwrap();
    assert!(
        again
            .mapping()
            .controls(runtime::io::key::A)
            .contains(&east)
    );
    // settings.conf is read from beside it.
    std::fs::write(dir.join("settings.conf"), "fast-speed = 3.5\n").unwrap();
    assert_eq!(
        settings::load(Some(&dir.join("settings.conf"))).map(|s| s.fast_speed),
        Ok(Some(3.5))
    );
    // Nothing went to the home or XDG folders.
    assert!(!home.exists() && !xdg.exists());
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// A ROM whose frames all differ: it turns the display on and counts into
/// the backdrop colour as fast as it can.
const COUNTING_ROM: [u32; 7] = [
    0xE3A02301, // mov r2, #0x04000000
    0xE3A03000, // mov r3, #0
    0xE1C230B0, // strh r3, [r2]       DISPCNT: mode 0, nothing on
    0xE3A01405, // mov r1, #0x05000000
    0xE2800001, // add r0, r0, #1
    0xE1C100B0, // strh r0, [r1]       palette 0, the backdrop
    0xEAFFFFFC, // b (the add)
];

/// Run `n` frames of [`COUNTING_ROM`] on the desktop `d` makes, giving
/// each frame's hash, the wall-clock time and how long the pacers held it
/// back ([`Desktop::paced`]). Wall-clock time only grows with the machine's
/// load, so it bounds a run from below; the pacers' time bounds it from
/// above. On a thread with room for the machine, which a debug build puts
/// on the stack before boxing it.
fn timed_frames(
    d: impl FnOnce() -> Desktop + Send + 'static,
    n: usize,
) -> (Vec<u64>, Duration, Duration) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let rom: Vec<u8> = COUNTING_ROM.iter().flat_map(|w| w.to_le_bytes()).collect();
            let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
            let mut m = Box::new(runtime::Machine::new(rom, d()));
            let t = std::time::Instant::now();
            let hashes = (0..n)
                .map(|_| {
                    m.run_frame();
                    frame_hash(&m.ppu.frame)
                })
                .collect();
            (hashes, t.elapsed(), m.platform.paced())
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn fast_forward_runs_fast_and_frames_match() {
    let n = 60;
    let gba = fast::frame();
    // Paced as the muted window is: 59.73 Hz.
    let (normal, t_normal, _) = timed_frames(|| Desktop::headless().with_clock(), n);
    assert!(t_normal >= gba * (n as u32 - 1), "{t_normal:?}");
    assert!(
        normal.windows(2).all(|w| w[0] != w[1]),
        "frames should differ"
    );
    // Unthrottled: never held back.
    let fast = || {
        Desktop::headless()
            .with_clock()
            .with_fast(fast::FastForward::new(0.0, true))
    };
    let (hashes, _, paced) = timed_frames(fast, n);
    assert_eq!(hashes, normal);
    assert_eq!(paced, Duration::ZERO);
    // At 4x: paced, but never past a quarter of the time.
    let fast = || {
        Desktop::headless()
            .with_clock()
            .with_fast(fast::FastForward::new(4.0, true))
    };
    let (hashes, t, paced) = timed_frames(fast, n);
    assert_eq!(hashes, normal);
    let quarter = gba * (n as u32 - 1) / 4;
    assert!(t >= quarter, "{t:?}");
    assert!(paced <= quarter, "{paced:?}");
}

#[test]
fn fast_forward_mutes_and_sound_comes_back() {
    let rate = 48000;
    let d = Desktop::headless()
        .with_player(audio::Player::undrained(rate))
        .with_fast(fast::FastForward::new(0.0, true));
    let rom: &'static [u8] = &[0xFE, 0xFF, 0xFF, 0xEA];
    let mut m = Box::new(runtime::Machine::new(rom, d));
    for _ in 0..60 {
        m.run_frame();
    }
    // Nothing queued, and no pacing: the device would play silence.
    assert_eq!(m.platform.paced(), Duration::ZERO);
    assert_eq!(m.platform.player.as_ref().unwrap().queue().len(), 0);
    // Released: a frame's sound is queued again (a silent frame, 804
    // samples at 48 kHz), and the queue fills before it plays.
    m.platform.fast.toggled = false;
    m.run_frame();
    let q = m.platform.player.as_ref().unwrap().queue();
    assert!((800..=810).contains(&q.len()), "{}", q.len());
    assert!(!q.started);
}

#[test]
fn state_options() {
    let o =
        parse("a.gba --headless --frames 20 --state-out s --state-at 5 --frame-hashes h").unwrap();
    assert_eq!(
        (o.state_out, o.state_at, o.frame_hashes),
        (Some("s".into()), Some(5), Some("h".into()))
    );
    assert!(parse("a.gba --headless --state-at 5").is_err());
    assert!(parse("a.gba --headless --frames 4 --state-out s --state-at 5").is_err());
    assert!(parse("a.gba --headless --state-out s --state-at 0").is_err());
}

#[test]
fn bench_options() {
    let o = parse("a.gba --headless --frame-times t --state-bench 50").unwrap();
    assert_eq!((o.frame_times, o.state_bench), (Some("t".into()), Some(50)));
    assert!(parse("a.gba --frame-times t").is_err());
    assert!(parse("a.gba --state-bench 5").is_err());
    assert!(parse("a.gba --headless --state-bench 0").is_err());
}

#[test]
fn state_bench_leaves_the_machine_as_it_was() {
    let rom: &'static [u8] = Box::leak(COUNTING_ROM.iter().flat_map(|w| w.to_le_bytes()).collect());
    let mut m = Box::new(runtime::Machine::new(rom, Desktop::headless()));
    m.run_frame();
    let before = slots::state(&mut m);
    let said = state_bench(&mut m, 3).unwrap();
    assert!(said.starts_with("state-bench x3: whole "), "{said}");
    assert!(slots::state(&mut m) == before);
}

#[test]
fn dump_at_options() {
    let o = parse("a.gba --headless --frames 20 --dump-at 5,20 --dump-dir d").unwrap();
    assert_eq!((o.dump_at, o.dump_dir), (vec![5, 20], Some("d".into())));
    assert!(parse("a.gba --headless --frames 20 --dump-at 5").is_err());
    assert!(parse("a.gba --headless --frames 20 --dump-dir d").is_err());
    assert!(parse("a.gba --headless --frames 20 --dump-at 21 --dump-dir d").is_err());
    assert!(parse("a.gba --headless --frames 20 --dump-at 0 --dump-dir d").is_err());
    assert!(parse("a.gba --frames 20 --dump-at 5 --dump-dir d").is_err());
}

#[test]
fn pad_buttons_save_and_load_the_last_slot() {
    use gamepad::PadEvent::*;
    use sdl3::gamepad::Button;
    use slots::Action::*;
    // No pad saves or loads a state by default; west and north do once
    // bound.
    let mut pads = gamepad::Pads::new(gamepad::Mapping::default());
    for b in [Button::West, Button::North] {
        pads.feed(0, Pressed(b));
        assert_eq!(pads.keys() & (gamepad::SAVE_STATE | gamepad::LOAD_STATE), 0);
        pads.feed(0, Released(b));
    }
    let m = gamepad::Mapping::default()
        .with("save-state = west\nload-state = north\n")
        .unwrap();
    let mut pads = gamepad::Pads::new(m);
    let mut s = slots::Slots::default();
    pads.feed(0, Pressed(Button::West));
    s.input(pads.keys(), None, false);
    assert_eq!(s.take(), Some(Save(1)));
    // Held isn't pressed again; and it's taken once.
    s.input(pads.keys(), None, false);
    assert_eq!(s.take(), None);
    // A slot key moves the pad to that slot.
    s.input(pads.keys(), Some((3, false)), false);
    assert_eq!(s.take(), Some(Load(3)));
    pads.feed(0, Released(Button::West));
    pads.feed(0, Pressed(Button::North));
    s.input(pads.keys(), None, false);
    assert_eq!(s.take(), Some(Load(3)));
    s.input(pads.keys(), Some((2, true)), false);
    assert_eq!(s.take(), Some(Save(2)));
    // F5 or the menu undoes the last load, and leaves the slot be.
    s.input(pads.keys(), None, true);
    assert_eq!(s.take(), Some(UndoLoad));
    pads.feed(0, Released(Button::North));
    s.input(pads.keys(), None, false);
    pads.feed(0, Pressed(Button::North));
    s.input(pads.keys(), None, false);
    assert_eq!(s.take(), Some(Load(2)));
}

#[test]
fn slot_states_round_trip_through_files() {
    // On a thread with room for the machines, which a debug build puts on
    // the stack before boxing them.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(slot_states)
        .unwrap()
        .join()
        .unwrap();
}

fn slot_states() {
    let rom: &'static [u8] = Box::leak(COUNTING_ROM.iter().flat_map(|w| w.to_le_bytes()).collect());
    let machine = || Box::new(runtime::Machine::new(rom, Desktop::headless()));
    let dir = std::env::temp_dir().join(format!("gbaa-slots-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let game = dir.join("game.gba");
    assert_eq!(slots::path(&game, 2), dir.join("game.state2"));

    let mut m = machine();
    m.run_frame();
    let mut n = machine();
    let undo = &mut None;
    let trial = &mut None;
    let mut act = |m: &mut DesktopMachine, a| slots::act(m, &game, a, undo, trial);
    assert_eq!(act(&mut n, slots::Action::Load(2)), "state 2 is empty");
    assert_eq!(act(&mut n, slots::Action::UndoLoad), "no load to undo");
    assert_eq!(act(&mut m, slots::Action::Save(2)), "saved state 2");
    let later: Vec<_> = (0..3).map(|_| (m.run_frame(), m.ppu.frame).1).collect();
    // n plays on past m's state; undoing the load comes back to here.
    (0..5).for_each(|_| n.run_frame());
    let n_before = slots::state(&mut n);
    assert_eq!(act(&mut n, slots::Action::Load(2)), "loaded state 2");
    for f in &later {
        n.run_frame();
        assert!(n.ppu.frame == *f);
    }
    assert_eq!(act(&mut n, slots::Action::UndoLoad), "load undone");
    // It plays on as a machine started from that moment does.
    let mut p = machine();
    p.load_state(&n_before).unwrap();
    for _ in 0..3 {
        n.run_frame();
        p.run_frame();
        assert!(n.ppu.frame == p.ppu.frame);
    }
    assert!(n.ppu.frame != later[2]);
    // Once only.
    assert_eq!(act(&mut n, slots::Action::UndoLoad), "no load to undo");
    drop(act);
    // Another ROM's state is copied aside, then loads anyway.
    let theirs = slots::state(&mut m);
    std::fs::write(dir.join("other.state1"), &theirs).unwrap();
    // Another build of the same code: one more word at its end.
    let padded: &'static [u8] = Box::leak([rom, &[0; 4]].concat().into_boxed_slice());
    let mut o = Box::new(runtime::Machine::new(padded, Desktop::headless()));
    let trial = &mut None;
    let said = slots::act(
        &mut o,
        &dir.join("other.gba"),
        slots::Action::Load(1),
        &mut None,
        trial,
    );
    assert_eq!(said, "loaded state 1, from another ROM (kept as .bak)");
    assert_eq!(std::fs::read(dir.join("other.state1.bak")).unwrap(), theirs);
    assert_eq!(o.cpu, m.cpu);
    // It plays on: on trial for a second, then kept.
    for _ in 0..slots::TRIAL_FRAMES {
        assert!(trial.is_some());
        o.run_frame();
        assert_eq!(slots::watch(&mut o, trial), None);
    }
    assert!(trial.is_none() && o.fault.is_none());
    // One whose code here differs where it stands (this ROM's loop jumps
    // into the BIOS) crashes, and is undone; its .bak stays.
    let crash: &'static [u8] = Box::leak(
        COUNTING_ROM[..4]
            .iter()
            .chain(&[0xE3A0FC01; 3]) // mov pc, #0x100
            .flat_map(|w| w.to_le_bytes())
            .collect(),
    );
    let mut c = Box::new(runtime::Machine::new(crash, Desktop::headless()));
    c.run_frame();
    let before = slots::state(&mut c);
    std::fs::write(dir.join("crash.state3"), &theirs).unwrap();
    let undo = &mut None;
    let said = slots::act(
        &mut c,
        &dir.join("crash.gba"),
        slots::Action::Load(3),
        undo,
        trial,
    );
    assert_eq!(said, "loaded state 3, from another ROM (kept as .bak)");
    c.run_frame();
    assert!(c.fault.is_some());
    let said = slots::watch(&mut c, trial).unwrap();
    assert_eq!(
        said,
        "state 3 is from another ROM and crashed in frame 1 (reached BIOS address \
         0x00000100, which isn't emulated): load undone"
    );
    assert!(trial.is_none() && c.fault.is_none());
    // Marked to write out again, as any undone load is.
    assert!(c.bus.save.dirty);
    c.bus.save.dirty = false;
    assert_eq!(slots::state(&mut c), before);
    assert_eq!(std::fs::read(dir.join("crash.state3.bak")).unwrap(), theirs);
    // Saved again from this ROM, it loads as any other.
    let ours = slots::state(&mut o);
    std::fs::write(dir.join("other.state1"), &ours).unwrap();
    std::fs::remove_file(dir.join("other.state1.bak")).unwrap();
    assert_eq!(slots::load(&mut o, &dir.join("other.state1")), Ok(None));
    assert!(!dir.join("other.state1.bak").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn auto_states_resume_and_keep_the_newer_sav() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(auto_states)
        .unwrap()
        .join()
        .unwrap();
}

fn auto_states() {
    let rom: &'static [u8] = Box::leak(COUNTING_ROM.iter().flat_map(|w| w.to_le_bytes()).collect());
    let machine = || Box::new(runtime::Machine::new(rom, Desktop::headless()));
    let dir = std::env::temp_dir().join(format!("gbaa-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let game = dir.join("game");
    assert_eq!(resume::path(&game), dir.join("game.resume"));
    // Nothing yet: nothing to say, the machine as it was.
    let mut n = machine();
    assert_eq!(resume::load(&mut n, &game, None, &mut None), None);

    // None before it's due; one after a .sav write, and one on quit.
    let mut m = machine();
    let mut auto = resume::Auto::new(&game, true);
    m.run_frame();
    auto.frame(&mut m, true, false);
    m.run_frame();
    auto.frame(&mut m, true, true);
    let before = slots::state(&mut m);
    m.run_frame();
    m.bus.save.data[..4].copy_from_slice(b"SAVE");
    auto.finish(&mut m);
    assert!(!dir.join("game.resume.tmp").exists());
    let quit = slots::state(&mut m);
    assert_eq!(std::fs::read(resume::path(&game)).unwrap(), quit);
    // The .sav is older: the state's save is kept, and marked to write.
    std::fs::write(sav_file::beside(&game), [0u8; 4]).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let set_mtime = |p: &std::path::Path, t| {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap()
    };
    set_mtime(&sav_file::beside(&game), old);
    let said = resume::load(&mut n, &game, Some(&sav_file::beside(&game)), &mut None);
    assert_eq!(said.as_deref(), Some("resumed where you left off"));
    assert!(n.bus.save.dirty);
    n.bus.save.dirty = m.bus.save.dirty;
    assert_eq!(slots::state(&mut n), quit);
    // Each write keeps the one before; the newest damaged, that loads.
    resume::write(&game, &before).unwrap();
    resume::write(&game, &quit).unwrap();
    assert_eq!(std::fs::read(resume::old_path(&game)).unwrap(), before);
    std::fs::write(resume::path(&game), b"broken").unwrap();
    let mut n = machine();
    let said = resume::load(&mut n, &game, None, &mut None).unwrap();
    assert_eq!(said, "resumed from the auto state before the last");
    assert_eq!(slots::state(&mut n), before);
    // A .sav newer than the state: its save stays.
    std::fs::write(resume::path(&game), &quit).unwrap();
    set_mtime(&resume::path(&game), old);
    set_mtime(&sav_file::beside(&game), std::time::SystemTime::now());
    let mut n = machine();
    n.bus.save.data[..4].copy_from_slice(b"NEWR");
    let said = resume::load(&mut n, &game, Some(&sav_file::beside(&game)), &mut None).unwrap();
    assert_eq!(said, "resumed where you left off, with the newer .sav");
    assert_eq!(&n.bus.save.data[..4], b"NEWR");
    assert_eq!(n.cpu, m.cpu);
    // One from another ROM (another version of the game) is copied aside
    // and resumes anyway.
    let rom2: &'static [u8] = &rom[..rom.len() - 4];
    let mut o = Box::new(runtime::Machine::new(rom2, Desktop::headless()));
    let fresh = slots::state(&mut o);
    let trial = &mut None;
    let said = resume::load(&mut o, &game, None, trial).unwrap();
    assert_eq!(
        said,
        "resumed where you left off, from another ROM (kept as .bak)"
    );
    assert_eq!(std::fs::read(dir.join("game.resume.bak")).unwrap(), quit);
    assert_eq!(o.cpu, m.cpu);
    // On trial; if it crashes, the game starts afresh.
    assert!(trial.is_some());
    o.fault = Some(runtime::machine::Fault::Bios(0x1234));
    let said = slots::watch(&mut o, trial).unwrap();
    assert!(said.starts_with("the auto state is from another ROM and crashed in frame 1"));
    assert!(said.ends_with(": started afresh"), "{said}");
    o.bus.save.dirty = false;
    assert_eq!(slots::state(&mut o), fresh);

    // Turned off, they go.
    let mut auto = resume::Auto::new(&game, true);
    auto.set(false);
    auto.finish(&mut m);
    assert!(!resume::path(&game).exists() && !resume::old_path(&game).exists());
    // A run that faults sets its auto state aside, and takes none on quit.
    let mut auto = resume::Auto::new(&game, true);
    auto.frame(&mut m, true, true);
    m.fault = Some(runtime::machine::Fault::Bios(0x1234));
    auto.finish(&mut m);
    assert!(!resume::path(&game).exists() && !resume::old_path(&game).exists());
    assert_eq!(std::fs::read(resume::fault_path(&game)).unwrap(), quit);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rewind_deltas_round_trip() {
    use rewind::{delta, undelta};
    let base: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
    let mut target = base.clone();
    for i in [0, 1, 3, 100, 101, 102, 4000, 4999] {
        target[i] ^= 0x5a;
    }
    let d = delta(&base, &target);
    assert!(d.len() < 40, "{}", d.len());
    assert_eq!(undelta(&base, &d), target);
    assert_eq!(undelta(&base, &delta(&base, &base)), base);
    assert_eq!(delta(&base, &base).len(), 4);
    // Lengths that differ: the target whole.
    assert_eq!(undelta(&base, &delta(&base, &target[..10])), &target[..10]);
    assert_eq!(undelta(&base, &delta(&[], &target)), target);
}

#[test]
fn rewind_steps_back_and_keeps_to_its_cap() {
    let state = |n: u8| -> Vec<u8> { (0..1000).map(|i| if i % 50 == 0 { n } else { 0 }).collect() };
    let mut r = rewind::Rewind::new(1 << 20);
    for f in 0..10 {
        r.push(f, state(f as u8));
    }
    assert_eq!(r.len(), 9);
    for f in (5..9).rev() {
        assert_eq!(r.back(), Some((f, &state(f as u8)[..])));
    }
    // From frame 5 it runs on: the steps back from there are as before.
    r.push(6, state(66));
    assert_eq!(r.back(), Some((5, &state(5)[..])));
    for f in (0..5).rev() {
        assert_eq!(r.back(), Some((f, &state(f as u8)[..])));
    }
    assert_eq!(r.back(), None);
    // Under a cap, the oldest go.
    let mut r = rewind::Rewind::new(1000 + 10 * 60);
    for f in 0..100 {
        r.push(f, state(f as u8));
    }
    assert!(
        r.bytes() <= 1600 && r.len() > 5,
        "{} {}",
        r.bytes(),
        r.len()
    );
    let kept = r.len() as u64;
    while r.back().is_some() {}
    assert_eq!(r.back(), None);
    assert!(r.summary().starts_with("rewind: 0 frames back"), "{kept}");
}

#[test]
fn rewind_options_and_button() {
    use sdl3::gamepad::Axis;
    let o = parse("a.gba --headless --rewind 90+30 --rewind-mb 8").unwrap();
    assert_eq!((o.rewind, o.rewind_mb), (Some((90, 30)), 8));
    assert_eq!(parse("a.gba").unwrap().rewind_mb, rewind::DEFAULT_MB);
    for bad in [
        "a.gba --rewind 90+30",
        "a.gba --headless --rewind 90",
        "a.gba --headless --rewind 90+0",
        "a.gba --headless --rewind 90+3 --rewind-mb 0",
        "a.gba --rewind-mb x",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    let mut pads = gamepad::Pads::new(gamepad::Mapping::default());
    pads.feed(0, gamepad::PadEvent::Axis(Axis::RightX, -0.9));
    assert_eq!(pads.keys(), gamepad::REWIND);
}

#[test]
fn the_console_button_is_f9_and_no_pad_control() {
    use sdl3::gamepad::Axis;
    use sdl3::keyboard::Scancode;
    let m = gamepad::Mapping::default();
    let mut pads = gamepad::Pads::new(m.clone());
    pads.feed(0, gamepad::PadEvent::Axis(Axis::RightX, 0.9));
    assert_eq!(pads.keys(), 0);
    assert_eq!(m.pressed(|k| k == Scancode::F9), gamepad::CONSOLE);
    let m = m.with("console = rstick-right\n").unwrap();
    let mut pads = gamepad::Pads::new(m);
    pads.feed(0, gamepad::PadEvent::Axis(Axis::RightX, 0.9));
    assert_eq!(pads.keys(), gamepad::CONSOLE);
}

#[test]
fn script_view_keeps_the_newest_console_lines() {
    let mut v = console::ScriptView::default();
    v.add_console((0..console::CONSOLE_LINES + 3).map(|n| n.to_string()));
    assert_eq!(v.console.len(), console::CONSOLE_LINES);
    assert_eq!(v.console[0], "3");
}

#[test]
fn script_dirs_are_the_config_folders_then_each_loaded_scripts() {
    let loaded = [
        (1, PathBuf::from("/s/a/x.lua")),
        (2, PathBuf::from("/s/a/y.lua")),
        (3, PathBuf::from("z.lua")),
    ];
    assert_eq!(
        console::script_dirs(Some(Path::new("/c")), &loaded),
        [
            PathBuf::from("/c/scripts"),
            PathBuf::from("/s/a"),
            PathBuf::from(".")
        ]
    );
}

#[test]
fn rewound_runs_draw_what_the_first_run_drew() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(rewound_runs)
        .unwrap()
        .join()
        .unwrap();
}

fn rewound_runs() {
    let dir = std::env::temp_dir().join(format!("gbaa-rewind-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rom = dir.join("count.gba");
    let words: Vec<u8> = COUNTING_ROM.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(&rom, words).unwrap();
    let hashes = |name: &str, more: &str| {
        let out = dir.join(name);
        let args = format!(
            "{} --headless --frames 40 --frame-hashes {} {more}",
            rom.display(),
            out.display()
        );
        run(&parse(&args).unwrap(), None, None, &mut None).unwrap();
        let text = std::fs::read_to_string(out).unwrap();
        text.lines().map(String::from).collect::<Vec<_>>()
    };
    let straight = hashes("straight", "");
    // 30 frames run, 25 stepped back (shown: 29 to 5), then on from 6 to
    // 40; line k of the straight run is frame k + 1's.
    let rewound = hashes("rewound", "--rewind 30+25");
    assert_eq!(rewound.len(), 40 + 25 + 25);
    let back: Vec<_> = straight[4..29].iter().rev().cloned().collect();
    assert_eq!(rewound[30..55], back[..]);
    assert_eq!(rewound[55..], straight[5..]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fullscreen_fits_the_deck_at_five_centred() {
    assert_eq!(platform::fit(1280, 800), (5, 40, 0));
    assert_eq!(platform::fit(1920, 1080), (6, 240, 60));
    assert_eq!(platform::fit(240, 160), (1, 0, 0));
}

#[test]
fn a_resized_window_fits_the_largest_whole_scale_centred() {
    // Dragged wider than tall: the height limits it, bars left and right.
    assert_eq!(platform::fit(1000, 330), (2, 260, 5));
    // Taller than wide: the width limits it, bars top and bottom.
    assert_eq!(platform::fit(500, 900), (2, 10, 290));
    // Just short of the next scale stays at this one.
    assert_eq!(platform::fit(959, 640), (3, 119, 80));
    assert_eq!(platform::fit(960, 640), (4, 0, 0));
    // Odd margins round down, keeping the frame whole.
    assert_eq!(platform::fit(723, 481), (3, 1, 0));
    // Smaller than the frame (before SDL's minimum size applies): 1x, at the corner.
    assert_eq!(platform::fit(100, 50), (1, 0, 0));
}

#[test]
fn fullscreen_options() {
    assert!(parse("a.gba --fullscreen").unwrap().fullscreen);
    assert!(!parse("a.gba").unwrap().fullscreen);
    // The monitor says its size now, not a flag.
    assert!(parse("a.gba --fullscreen --screen 1920x1080").is_err());
    assert!(parse("a.gba --fullscreen --headless").is_err());
}

/// The Deck's Game Mode gives games Xwayland only, and Linux desktops are
/// Wayland or X11: a build of SDL without either driver (a missing
/// header: crates/desktop/include/README.md) would open no window there.
#[cfg(target_os = "linux")]
#[test]
fn sdl_has_x11_and_wayland() {
    let drivers: Vec<_> = sdl3::video::drivers().collect();
    for want in ["x11", "wayland"] {
        assert!(
            drivers.contains(&want),
            "SDL built without {want}: {drivers:?}"
        );
    }
}

/// The volume keys press no button by default, nor player 2's.
#[test]
fn the_volume_keys_are_free() {
    use crate::window::sound_key;
    use sdl3::keyboard::Scancode;
    let keys = [
        Scancode::Minus,
        Scancode::KpMinus,
        Scancode::Equals,
        Scancode::KpPlus,
        Scancode::M,
    ];
    for sc in keys {
        assert!(sound_key(sc).is_some(), "{sc:?}");
        assert_eq!(gamepad::Mapping::default().pressed(|k| k == sc), 0);
        assert_eq!(gamepad::player2_keys().pressed(|k| k == sc), 0);
    }
    assert_eq!(sound_key(Scancode::Z), None);
}

/// Steam's desktop controller layout sends Escape for the Deck's B: no
/// key a pad's face button sends quits, only Ctrl+Q.
#[test]
fn only_ctrl_q_quits() {
    use crate::window::quits;
    use sdl3::keyboard::{Mod, Scancode};
    assert!(quits(Scancode::Q, Mod::LCTRLMOD));
    assert!(quits(Scancode::Q, Mod::RCTRLMOD | Mod::LSHIFTMOD));
    assert!(!quits(Scancode::Q, Mod::NOMOD));
    assert_eq!(
        quits(Scancode::Q, Mod::LGUIMOD),
        cfg!(target_os = "macos"),
        "Cmd+Q, macOS's own"
    );
    for sc in [Scancode::Escape, Scancode::Return, Scancode::Space] {
        for m in [Mod::NOMOD, Mod::LCTRLMOD] {
            assert!(!quits(sc, m), "{sc:?} {m:?}");
        }
    }
}

/// F10 opens the menu; Escape does only while no pad is open, as Steam's
/// desktop layout sends Escape for a pad's B.
#[test]
fn escape_opens_the_menu_without_a_pad() {
    use crate::window::opens_menu;
    use sdl3::keyboard::Scancode;
    assert!(opens_menu(Scancode::F10, false) && opens_menu(Scancode::F10, true));
    assert!(opens_menu(Scancode::Escape, false));
    assert!(!opens_menu(Scancode::Escape, true));
    assert!(!opens_menu(Scancode::Return, false));
}

/// F11 or Alt+Enter switches fullscreen; Enter alone is Start.
#[test]
fn f11_and_alt_enter_switch_fullscreen() {
    use crate::window::switches_fullscreen;
    use sdl3::keyboard::{Mod, Scancode};
    assert!(switches_fullscreen(Scancode::F11, Mod::NOMOD));
    assert!(switches_fullscreen(Scancode::Return, Mod::LALTMOD));
    assert!(switches_fullscreen(Scancode::KpEnter, Mod::RALTMOD));
    assert!(!switches_fullscreen(Scancode::Return, Mod::NOMOD));
    assert!(!switches_fullscreen(Scancode::Return, Mod::LCTRLMOD));
    assert!(!switches_fullscreen(Scancode::F10, Mod::LALTMOD));
}

#[test]
fn full_script_access_is_remembered() {
    use script::Access;
    let dir = std::env::temp_dir().join(format!("gbaa-script-access-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
    std::fs::write(&a, "").unwrap();
    std::fs::write(&b, "").unwrap();
    let conf = dir.join("config/script-access.conf");
    let access = |p: &Path, asked| scripting::access(p, asked, Some(&conf)).unwrap();
    assert_eq!(access(&a, false), Access::Sandboxed);
    assert!(!conf.exists());
    assert_eq!(access(&a, true), Access::Full);
    assert_eq!(access(&dir.join("./a.lua"), false), Access::Full);
    assert_eq!(access(&b, false), Access::Sandboxed);
    assert_eq!(access(&b, true), Access::Full);
    let text = std::fs::read_to_string(&conf).unwrap();
    let full = scripting::full_scripts(&text).unwrap();
    assert_eq!(full, [a.canonicalize().unwrap(), b.canonicalize().unwrap()]);
    assert!(scripting::full_scripts("full /x").is_err());
    let o = parse("game.gba --script a.lua --script-full b.lua").unwrap();
    assert_eq!(
        o.scripts,
        [
            (PathBuf::from("a.lua"), false),
            (PathBuf::from("b.lua"), true)
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn input_scripts_repeat_a_press() {
    use runtime::io::key;
    let s = Script::parse("a@10-30/10+2").unwrap();
    let held: Vec<u64> = (0..40).filter(|&f| s.keys(f) == key::A).collect();
    assert_eq!(held, [10, 11, 20, 21, 30, 31]);
    for bad in ["a@30-10/5", "a@1-5/0", "a@1-5", "a@1-/2"] {
        assert!(Script::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn linked_play_options() {
    use runtime::io::key;
    let o = parse("g.gba --headless --link b.sav --input2 a@3").unwrap();
    assert_eq!(o.link, Some(PathBuf::from("b.sav")));
    assert_eq!(o.input2.keys(3), key::A);
    assert!(parse("g.gba --headless --input2 a@3").is_err());
    for other in [
        "--state s.ss",
        "--script x.lua",
        "--rewind 1+2",
        "--wav w.wav",
    ] {
        let e = parse(&format!("g.gba --headless --link b.sav {other}")).unwrap_err();
        assert!(e.contains("linked play"), "{other}: {e}");
    }
    // The window keeps player 1's save in the game's folder, as for one GBA.
    let o = parse("g.gba --link b.sav").unwrap();
    assert_eq!(o.save, Some(sav_in_folder("g")));
}

#[test]
fn netplay_options() {
    let o = parse("g.gba --server example.org --join ab3xyz").unwrap();
    assert_eq!(o.server.as_deref(), Some("example.org"));
    assert_eq!(o.role, Some(room::Role::Join(*b"AB3XYZ")));
    // The window keeps this player's save in the game's folder.
    assert_eq!(o.save, Some(sav_in_folder("g")));
    let o = parse("g.gba --headless --server 127.0.0.1:7374 --host --delay 5").unwrap();
    assert_eq!((o.role, o.delay), (Some(room::Role::Host), Some(5)));
    let o = parse("g.gba --headless --server s --host --players 4").unwrap();
    assert_eq!(o.players, Some(4));
    // A seat may start from a state, as a window's game goes on into its
    // session (multiplayer/join-without-restart); --state-out stays out.
    let o = parse("g.gba --headless --server s --join abcdef --state p2.state").unwrap();
    assert_eq!(o.state.as_deref(), Some(Path::new("p2.state")));
    let e = parse("g.gba --headless --server s --host --state-out x").unwrap_err();
    assert!(e.contains("--state-out"), "{e}");
    for (args, why) in [
        ("--server s --host --players 5", "--players"),
        ("--server s --host --players 1", "--players"),
        ("--server s --join abcdef --players 3", "host's"),
        ("--players 3", "with --server"),
        ("--host", "--server"),
        ("--server s", "--host or --join"),
        ("--server s --join abc", "room code"),
        ("--server s --join ab-xyz", "room code"),
        ("--server s --join abcdef --delay 4", "host's"),
        ("--server s --host --delay 31", "delay"),
        ("--paced", "with --server"),
        ("--server s --host --lockstep", "--lockstep"),
        ("--server s --host --link b.sav", "--link"),
        ("--server s --host --rewind 1+2", "netplay"),
        ("--server s --host --fast-forward", "netplay"),
        ("--delay 3", "with --server"),
    ] {
        let e = parse(&format!("g.gba {args}")).unwrap_err();
        assert!(e.contains(why), "{args}: {e}");
    }
}

#[test]
fn pads_take_seats_in_the_order_they_came() {
    use gamepad::PadEvent::*;
    use runtime::io::key;
    use sdl3::gamepad::Button;
    let mut pads = gamepad::Pads::new(gamepad::Mapping::default());
    pads.feed(7, Pressed(Button::South));
    pads.feed(3, Pressed(Button::East));
    assert_eq!((pads.keys_of(0), pads.keys_of(1)), (key::A, key::B));
    // The first unplugged: the second moves up to player 1.
    pads.feed(7, Gone);
    assert_eq!((pads.keys_of(0), pads.keys_of(1)), (key::B, 0));
}

#[test]
fn player_two_keys_are_their_own() {
    use runtime::io::key;
    use sdl3::keyboard::Scancode;
    let p2 = gamepad::player2_keys();
    let held = |ks: &[Scancode]| p2.pressed(|k| ks.contains(&k));
    assert_eq!(held(&[Scancode::I, Scancode::N]), key::UP | key::A);
    assert_eq!(
        held(&[Scancode::Period, Scancode::Comma]),
        key::START | key::SELECT
    );
    // None of player 1's default keys press anything for player 2.
    let p1 = gamepad::Mapping::default();
    for bit in gamepad::BUTTONS.iter().map(|b| b.1) {
        for c in p1.keys(bit) {
            let gamepad::Control::Key(k) = c else {
                continue;
            };
            assert_eq!(held(&[k]), 0, "{k:?}");
        }
    }
}

#[test]
fn two_screens_side_by_side() {
    let (mut a, mut b) = (
        Box::new([0u16; WIDTH * HEIGHT]),
        Box::new([0u16; WIDTH * HEIGHT]),
    );
    a[WIDTH + 5] = 1;
    b[2 * WIDTH - 1] = 2;
    let mut out = vec![0; 2 * WIDTH * HEIGHT];
    window::side_by_side(&[&a, &b], &mut out);
    assert_eq!(out[2 * WIDTH + 5], 1);
    assert_eq!(out[4 * WIDTH - 1], 2);
    assert_eq!(out.iter().filter(|&&c| c != 0).count(), 2);
    assert_eq!(platform::fit_wide(1440, 480, 2 * WIDTH), (3, 0, 0));
    assert_eq!(platform::fit_wide(1280, 800, 2 * WIDTH), (2, 160, 240));
}

#[test]
fn linked_play_runs_headless() {
    let dir = std::env::temp_dir().join(format!("gbaa-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // `b .`, forever.
    let rom: Vec<u8> = 0xEAFF_FFFEu32.to_le_bytes().repeat(64);
    std::fs::write(dir.join("loop.gba"), rom).unwrap();
    let o = parse(&format!(
        "{0}/loop.gba --headless --frames 3 --link {0}/b.sav --dump {0}/both.png",
        dir.display()
    ))
    .unwrap();
    // Two machines want more stack than a test thread's in a debug build.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || link::run(&o, o.timing(None), None))
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
    let png = std::fs::read(dir.join("both.png")).unwrap();
    // The IHDR's width: both screens.
    assert_eq!(&png[16..20], &(2 * WIDTH as u32).to_be_bytes());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `--assets-mod`: a raw blob's override patches a copy of the ROM and
/// keeps the original's CRC to report; a compressed one goes to the BIOS
/// calls' table; a recompiled build refuses a patch over bytes its code
/// folded in.
#[test]
fn asset_overrides_patch_a_copy_and_refuse_folded_bytes() {
    let dir = std::env::temp_dir().join(format!("gbaa-assets-mod-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut rom = vec![0u8; 0x200];
    rom[0x100..0x104].copy_from_slice(&[0x1F, 0x00, 0xE0, 0x03]);
    rom[0x180..0x188].copy_from_slice(&[0x30, 4, 0, 0, 0x81, 7, 0, 0]);
    let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
    std::fs::write(
        dir.join("manifest.toml"),
        "[palettes]\n0x08000100 = { store = \"raw\", size = 4, colours = 2 }\n\
         [tilemaps]\n0x08000180 = { store = \"rl\", size = 8, unpacked = 4 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("p.pal"),
        "JASC-PAL\n0100\n2\n0 0 248\n248 248 248\n",
    )
    .unwrap();
    std::fs::write(dir.join("m.bin"), [1, 2, 3, 4]).unwrap();
    std::fs::write(
        dir.join("mod.toml"),
        "[assets]\n0x08000100 = \"p.pal\"\n0x08000180 = \"m.bin\"\n",
    )
    .unwrap();
    let (m, manifest) = (dir.as_path(), dir.join("manifest.toml"));
    let (patched, table, crc) = asset_overrides(rom, m, &manifest, None).unwrap();
    assert_eq!(patched[0x100..0x104], [0x00, 0x7C, 0xFF, 0x7F]);
    assert_eq!(patched[..0x100], rom[..0x100]);
    assert_eq!(crc, Some(runtime::state::crc32(rom)));
    assert_eq!(table, [(0x0800_0180, &[1u8, 2, 3, 4][..])]);
    static FOLDED: Code = Code {
        blocks: &[],
        ram_blocks: &[],
        overlays: &[],
        extents: &[],
        ram_extents: &[],
        inner: &[],
        inner_extents: &[],
        modded: &[],
        folded: &[(0x0800_0102, 0x0800_0104)],
        rom_hash: 0,
        timing: runtime::Timing::Exact,
        seeds_file: None,
        spins: &[],
    };
    let e = asset_overrides(rom, m, &manifest, Some(&FOLDED)).unwrap_err();
    assert!(e.contains("0x08000100"), "{e}");
    assert!(e.contains("0x08000104, which this"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_desktop_seat_computes_what_a_pad_does() {
    use netplay::Console;
    use netplay::demo::{key_folder, script};
    let rom = key_folder();
    let mut pad = netplay::gba::Gba::new(rom, &[None, None]);
    let mut desk =
        netplay::gba::Gba::<Desktop>::with_code(rom, &[None, None], None, runtime::Timing::Exact);
    assert_eq!(pad.build_id(), desk.build_id());
    for f in 0..60 {
        let keys = [script(0, f), script(1, f)];
        pad.step(&keys).unwrap();
        desk.step(&keys).unwrap();
    }
    assert_eq!(pad.hash(), desk.hash());
}

#[test]
fn netplay_guests_compute_what_unwinding_does_and_roll_back() {
    use netplay::demo::{key_folder, script};
    use netplay::{Console, Snapshot};
    static NONE: Code = Code {
        blocks: &[],
        ram_blocks: &[],
        overlays: &[],
        extents: &[],
        ram_extents: &[],
        inner: &[],
        inner_extents: &[],
        modded: &[],
        folded: &[],
        rom_hash: 0,
        timing: runtime::Timing::Exact,
        seeds_file: None,
        spins: &[],
    };
    let rom = key_folder();
    let mut pad = netplay::gba::Gba::new(rom, &[None, None]);
    let mut g = netplay::gba::Gba::<Desktop>::with_code(
        rom,
        &[None, None],
        Some(&NONE),
        runtime::Timing::Exact,
    );
    g.set_turns(Box::<coro::Guests>::default());
    let mut snap = Default::default();
    let mut at = 0;
    for f in 0..90 {
        if f == 30 {
            g.snapshot(&mut snap);
            at = g.hash();
        }
        let keys = [script(0, f), script(1, f)];
        pad.step(&keys).unwrap();
        g.step(&keys).unwrap();
    }
    // Every turn of the cable switched out instead of unwinding, and the
    // keys were read at each V-blank.
    assert!(g.stats().switches > 2 * 90 * 200, "{}", g.stats().switches);
    let end = g.hash();
    assert_eq!(pad.hash(), end);
    g.restore(&snap).unwrap();
    assert_eq!(g.hash(), at);
    for f in 30..90 {
        g.step(&[script(0, f), script(1, f)]).unwrap();
    }
    assert_eq!(g.hash(), end);
}

/// Netplay's pacer catches a late frame up, as sound pacing does, and a
/// hold puts the next frame a period later.
#[test]
fn the_netplay_pacer_catches_up_and_holds() {
    use crate::fast::Pacer;
    use std::time::{Duration, Instant};
    let period = Duration::from_millis(10);
    let mut p = Pacer::default();
    p.wait(period);
    std::thread::sleep(Duration::from_millis(30));
    // Two frames are overdue: neither waits (a plain clock would wait one).
    let t = Instant::now();
    p.wait(period);
    p.wait(period);
    assert!(t.elapsed() < Duration::from_millis(8), "{:?}", t.elapsed());
    // On time again; a hold makes the next wait two periods.
    p.wait(period);
    p.hold(period);
    let t = Instant::now();
    p.wait(period);
    assert!(
        t.elapsed() >= Duration::from_millis(19),
        "{:?}",
        t.elapsed()
    );
}
