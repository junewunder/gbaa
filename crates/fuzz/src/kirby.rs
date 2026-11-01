//! Kirby & The Amazing Mirror's worker (assets/kirby-fuzz): the fuzzer
//! ported to a game with no save editor and no pret. Its states come from
//! play and RAM pokes, its addresses from the katam decomp's RAM layout
//! (`include/kirby.h`, `linker.ld`) and function names, which match this
//! ROM byte for byte (memory/kirby-amazing-mirror.md).
//!
//! - **Route.** From reset with a blank cartridge, the presses that start
//!   a new file and reach the first room (`just kirby-assets`'s), then the
//!   timing lab's walk on to frame 4500 (`just timing-lab`'s). States
//!   are kept at the title, the file select, the first room and along
//!   the walk. What it finds is the *route* phase.
//! - **Room sweep**, each worker taking every `of`-th of the 983 rooms in
//!   `gRoomProps`: from the first room, Kirby sent through a door to the
//!   room (`roomId` and `spawnLocation` written, then the door's own entry,
//!   `sub_0805545C`, called), and 600 frames of a walker's presses.
//! - **Ability sweep**: each of the 26 copy abilities set on Kirby in the
//!   first room and the ability's own loaders called (as the game does
//!   when it gives one back, `sub_0806F260`, `sub_0806EFF8`,
//!   `sub_0806F288`), then 600 frames of a masher's presses (B most often
//!   after A: the ability's attacks).
//! - **Screen sweep** (assets/kirby-fuzz-screens): each screen the rooms
//!   don't open, in each of the six languages. A screen is reached the way
//!   the game leaves a room for one (its game over, its ending): the fade
//!   `sub_08020428` with the screen's creator as its callback, which
//!   destroys every task and resets the heaps first. A creator that takes
//!   an argument (the sub-games' `sub_0801A744(n)`) is faded to a no-op
//!   and then called. The screens: the logo, the title, the intro
//!   cutscene, the file select, the main menu as a sub-game leaves it,
//!   each of `sub_0801A744`'s four games (Speed Eaters, Crackity Hack,
//!   Kirby Wave Ride; katam doesn't name the fourth), the ending, the
//!   multiplayer ending, game over, the link screen, the first room with
//!   the pause menu, and the collection room with every treasure set;
//!   then the pause menu's help with each ability. The language
//!   is `gLanguage`, a constant in ROM (English, 1), so the worker plays a
//!   copy of the ROM with it patched (keeping the original's CRC, so states
//!   load either way), then puts the original back.
//! - **Fuzzing**, the shared core's: a kept state picked, maybe poked (a
//!   warp to a random room, a random ability, health and lives refilled,
//!   or random bits of the visited doors and treasures that the pause
//!   map and the collection room draw from), random presses.
//!
//! The screen column is Kirby's `stateFn`; the place is his room.

use std::path::Path;

use emerald_save::Presses as Script;
use runtime::Machine;
use runtime::io::key;

use crate::core::{self, Core, Pad, Presses};

/// `gKirbys[0]`, player 1's `struct Kirby` (0x1A8 bytes), and its fields.
const KIRBY: u32 = 0x0202_0EE0;
const STATE_FN: u32 = 0x78;
const ROOM: u32 = 0x60;
const HP: u32 = 0x100;
const MAX_HP: u32 = 0x101;
const LIVES: u32 = 0x102;
const ABILITY: u32 = 0x103;
/// The room a door sends him to, and where in it.
const ROOM_TO: u32 = 0x106;
const SPAWN: u32 = 0x108;
/// `gRoomProps` (0x089331AC to 0x0893CB44, 0x28 bytes a room).
const ROOMS: u32 = 983;
/// `sub_0805545C`: Kirby goes through a door to `roomId`.
const ENTER_DOOR: u32 = 0x0805_545D;
/// What the game calls when it gives an ability back
/// (`sub_0806F260`, `sub_0806EFF8`, `sub_0806F288`), and Cook's, Crash's
/// and Magic's extra (`sub_08088118`).
const ABILITY_LOADERS: [u32; 3] = [0x0806_F261, 0x0806_EFF9, 0x0806_F289];
const ABILITY_EXTRA: u32 = 0x0808_8119;
const ABILITY_EXTRA_FOR: [u8; 3] = [12, 21, 24];
/// `KIRBY_ABILITY_COUNT`: normal, then 26.
const ABILITIES: u32 = 27;
/// `gVisitedDoors` (to 0x02028EE0) and `gTreasures` (to 0x02038990).
const VISITED_DOORS: (u32, u32) = (0x0202_8CA0, 0x240);
const TREASURES: (u32, u32) = (0x0203_8960, 0x30);

/// `sub_08020428`: fade out, destroy every task, reset the heaps and call
/// the callback (how the game goes from a room to game over or the ending).
const FADE_TO: u32 = 0x0802_0429;
/// `nullsub_101`, a callback that does nothing.
const NOP: u32 = 0x0800_DC55;
/// Frames the fade takes before its callback has run (about 20).
const FADE_FRAMES: u64 = 30;
/// `gLanguage`, a halfword constant in ROM, and `NUM_LANGUAGES`
/// (Japanese, English, German, French, Spanish, Italian).
const LANGUAGE: usize = 0x00D6_0A80;
const LANGUAGES: u16 = 6;

/// How a screen item is played.
#[derive(Clone, Copy)]
enum Keys {
    /// No presses (a cutscene, which a press may skip).
    Still,
    /// A masher's presses: menus, sub-games.
    Mash,
    /// After a second, a tap every 20 frames: RIGHT, every fifth DOWN,
    /// every seventh A. The collection room's treasures, one by one.
    Browse,
    /// START held a moment, two seconds still for the menu to draw, then a
    /// masher's presses: the pause menu.
    Pause,
}

/// A screen the rooms don't open: the creator the fade calls (with
/// `arg`, called after the fade, if it takes one), or 0 to stay in the
/// first room; the frames it plays and how.
struct Screen {
    name: &'static str,
    create: u32,
    arg: Option<u32>,
    frames: u64,
    keys: Keys,
}

/// The collection room (`sub_081434F8`): with every treasure in
/// `gTreasures` set first, so its text strips (per treasure and language)
/// all have something to show.
const COLLECTION: u32 = 0x0814_34F9;

const fn screen_item(
    name: &'static str,
    create: u32,
    arg: Option<u32>,
    frames: u64,
    keys: Keys,
) -> Screen {
    Screen {
        name,
        create,
        arg,
        frames,
        keys,
    }
}

/// The screens, from the katam decomp's callers of the fade and of the
/// task creators (`demo.c`, `title_screen.c`, `code_08138D64.c`,
/// `multi_08019F28.c`, `dark_mind.c`, `kirby.c`, `multi_08032E98.c`).
const SCREENS: [Screen; 17] = [
    screen_item("logo", 0x0813_87B1, None, 1200, Keys::Still),
    screen_item("title", 0x0814_9CE5, None, 1800, Keys::Mash),
    // `CreateCutscene`: the intro, the mirror broken.
    screen_item("intro", 0x0802_6045, None, 3600, Keys::Still),
    // `sub_08138B44`: the file select and its menu, as the title opens it.
    screen_item("file select", 0x0813_8B45, None, 1800, Keys::Mash),
    // `sub_08138D64(n)`: the same menu, back from sub-game n.
    screen_item("menu 0", 0x0813_8D65, Some(0), 1200, Keys::Mash),
    screen_item("menu 1", 0x0813_8D65, Some(1), 1200, Keys::Mash),
    screen_item("menu 2", 0x0813_8D65, Some(2), 1200, Keys::Mash),
    // `sub_0801A744(n)`: a sub-game for one player against the computer
    // (Speed Eaters, Crackity Hack, Kirby Wave Ride, and a fourth katam
    // doesn't name): its menu, its instructions, the game and its results.
    screen_item("sub-game 0", 0x0801_A745, Some(0), 5400, Keys::Mash),
    screen_item("sub-game 1", 0x0801_A745, Some(1), 5400, Keys::Mash),
    screen_item("sub-game 2", 0x0801_A745, Some(2), 5400, Keys::Mash),
    screen_item("sub-game 3", 0x0801_A745, Some(3), 5400, Keys::Mash),
    // Dark Mind beaten: `sub_0802B4A8` alone, `sub_08025A80` with
    // friends (`gUnk_0203AD10 & 4`).
    screen_item("ending", 0x0802_B4A9, None, 7200, Keys::Still),
    screen_item("ending, multiplayer", 0x0802_5A81, None, 3600, Keys::Still),
    // Every Kirby out of health.
    screen_item("game over", 0x0802_5AD1, None, 1200, Keys::Mash),
    // `sub_08032E98`: the link screen.
    screen_item("link", 0x0803_2E99, None, 1200, Keys::Mash),
    screen_item("pause", 0, None, 1200, Keys::Pause),
    screen_item("collection", COLLECTION, None, 5400, Keys::Browse),
];

/// The pause menu's help, with the ability just given.
const HELP: Screen = screen_item("help", 0, None, 300, Keys::Pause);

/// A new file to the first room (`just kirby-assets`), then the timing
/// lab's walk, floats and puffs (`just timing-lab`).
const ROUTE: &str = "start@400,start@700,a@800,a@950,a@1100,a@1300,a@1500,\
right@1700+60,right@1800+3,right@1806+45,a@1900+3,a@1960+30,a@2050+3,a@2062+3,\
a@2074+3,a@2086+3,a@2098+40,b@2200+3,down@2280+30,down@2350+3,a@2353+3,b@2420+50,\
left@2520+20,a@2540+3,left@2543+20,right@2650+120,a@2700+30,a@2780+3,a@2792+3,\
right@3000+1500,a@3000-3240/12+4,b@3300+3,a@3400-3600/12+4,b@3650+3,a@3800+3,\
a@3900-4100/12+4,b@4150+3,a@4250+20,b@4350+40";
const ROUTE_END: u64 = 4500;
/// The title, the file select, the first room once Kirby obeys the pad,
/// and along the walk.
const ROUTE_KEEP: [u64; 6] = [400, 700, 1100, 1700, 3000, 4500];
/// The first room, where sweeps start.
const FIRST_ROOM: u64 = 1700;
/// Frames a sweep item plays.
const SWEEP_FRAMES: u64 = 600;

/// The screen column: Kirby's state function.
fn screen(m: &Machine<Pad>) -> u32 {
    m.bus.read32(KIRBY + STATE_FN)
}

/// The place: Kirby's room.
fn place(m: &Machine<Pad>) -> String {
    m.bus.read16(KIRBY + ROOM).to_string()
}

/// Send Kirby through a door to `room`, at `(x, y)` (in 16-pixel blocks).
fn warp(c: &mut Core, room: u32, x: u16, y: u16) -> bool {
    c.m.bus.write16(KIRBY + ROOM_TO, room as u16);
    c.m.bus.write16(KIRBY + SPAWN, x);
    c.m.bus.write16(KIRBY + SPAWN + 2, y);
    core::call(&mut c.m, ENTER_DOOR, &[KIRBY])
}

/// Give Kirby ability `a` and load its graphics as the game does.
fn give(c: &mut Core, a: u8) -> bool {
    c.m.bus.write8(KIRBY + ABILITY, a);
    let mut ok = true;
    for f in ABILITY_LOADERS {
        ok &= core::call(&mut c.m, f, &[KIRBY]);
    }
    if ABILITY_EXTRA_FOR.contains(&a) {
        ok &= core::call(&mut c.m, ABILITY_EXTRA, &[KIRBY]);
    }
    ok
}

/// Health and lives full.
fn refill(c: &mut Core) {
    let max = c.m.bus.read8(KIRBY + MAX_HP).max(6);
    c.m.bus.write8(KIRBY + HP, max);
    c.m.bus.write8(KIRBY + LIVES, 9);
}

/// The game in RAM, poked: one of a warp, an ability, a refill, or the
/// visited doors and treasures.
fn poke(c: &mut Core) {
    c.pokes += 1;
    match c.rng.below(4) {
        0 => {
            let room = c.rng.below(ROOMS);
            let (x, y) = (2 + c.rng.below(12) as u16, 2 + c.rng.below(10) as u16);
            warp(c, room, x, y);
        }
        1 => {
            let a = 1 + c.rng.below(ABILITIES - 1) as u8;
            give(c, a);
        }
        2 => refill(c),
        _ => {
            for (at, len) in [VISITED_DOORS, TREASURES] {
                for _ in 0..1 + c.rng.below(32) {
                    let a = at + c.rng.below(len);
                    let v = c.m.bus.read8(a) | 1 << c.rng.below(8);
                    c.m.bus.write8(a, v);
                }
            }
        }
    }
}

/// Play a sweep item from the state just set up: `frames` of a walker's
/// presses (or a masher's), keeping the states that find something.
fn sweep_play(c: &mut Core, walker: bool, why: &str) {
    let mut p = Presses {
        walker,
        held: 0,
        left: 0,
        gap: 0,
    };
    c.sweeps += 1;
    c.play(SWEEP_FRAMES, 0, why, |_, r| p.keys(r), &[]);
}

/// The ROM with `gLanguage` set to `lang`: a leaked copy, made once.
fn rom_in(
    copies: &mut Vec<(u16, &'static [u8])>,
    english: &'static [u8],
    lang: u16,
) -> &'static [u8] {
    if english.get(LANGUAGE..LANGUAGE + 2) == Some(&lang.to_le_bytes()[..]) {
        return english;
    }
    if let Some(&(_, r)) = copies.iter().find(|(l, _)| *l == lang) {
        return r;
    }
    let mut r = english.to_vec();
    r[LANGUAGE..LANGUAGE + 2].copy_from_slice(&lang.to_le_bytes());
    let r: &'static [u8] = Box::leak(r.into_boxed_slice());
    copies.push((lang, r));
    r
}

/// Play screen item `s` from the first room, in the ROM now on the bus.
fn screen_play(c: &mut Core, s: &Screen) {
    let mut p = Presses {
        walker: false,
        held: 0,
        left: 0,
        gap: 0,
    };
    let why = format!("screen {}", s.name);
    let mut frames = s.frames;
    if s.create == COLLECTION {
        // Chests, shards, spray paints, music and big chests; the four
        // vitality hearts.
        // (`struct Treasures`: 0x20 bytes of bits, then the hearts' u16.)
        let at = TREASURES.0;
        for a in at..at + 0x20 {
            c.m.bus.write8(a, 0xFF);
        }
        c.m.bus.write16(at + 0x20, 0xF);
    }
    if s.create != 0 {
        let callback = if s.arg.is_some() { NOP } else { s.create };
        if !core::call(&mut c.m, FADE_TO, &[callback]) {
            c.faults += 1;
            return;
        }
        if let Some(arg) = s.arg {
            if c.play(FADE_FRAMES, 0, &why, |_, _| 0, &[]).is_none() {
                return;
            }
            frames -= FADE_FRAMES;
            if !core::call(&mut c.m, s.create, &[arg]) {
                c.faults += 1;
                return;
            }
        }
    }
    c.sweeps += 1;
    let keys = s.keys;
    c.play(
        frames,
        0,
        &why,
        |f, r| match keys {
            Keys::Still => 0,
            Keys::Browse if f < 60 || f % 20 > 1 => 0,
            Keys::Browse if (f / 20) % 7 == 6 => key::A,
            Keys::Browse if (f / 20) % 5 == 4 => key::DOWN,
            Keys::Browse => key::RIGHT,
            Keys::Pause if f < 10 => 0,
            Keys::Pause if f < 14 => key::START,
            Keys::Pause if f < 120 => 0,
            _ => p.keys(r),
        },
        &[],
    );
}

/// `gbaa-fuzz worker` on Kirby: run for `secs` seconds into `out`.
pub fn run(
    rom_path: &Path,
    out: &Path,
    secs: u64,
    seed: u64,
    index: u32,
    of: u32,
) -> Result<(), String> {
    let rom = std::fs::read(rom_path).map_err(|e| format!("{}: {e}", rom_path.display()))?;
    let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
    let mut c = Core::new(rom, out, seed, index, of, screen, place)?;
    let route = Script::parse(ROUTE).map_err(|e| format!("the route: {e}"))?;
    c.play(ROUTE_END, 0, "route", |f, _| route.keys(f + 1), &ROUTE_KEEP);
    // The first room's state, kept by the route at FIRST_ROOM.
    let first = c
        .corpus
        .get(ROUTE_KEEP.iter().position(|&f| f == FIRST_ROOM).unwrap())
        .ok_or("the route kept no state in the first room")?
        .state
        .clone();
    c.cov.phase = "room";
    let mut r = c.index;
    while r < ROOMS && c.cov.start.elapsed().as_secs() < secs {
        c.m.load_state(&first).expect("our own state loads");
        refill(&mut c);
        warp(&mut c, r, 4, 4);
        sweep_play(&mut c, true, "room");
        c.tick();
        r += c.of;
    }
    c.cov.phase = "ability";
    let mut a = 1 + c.index;
    while a < ABILITIES && c.cov.start.elapsed().as_secs() < secs {
        c.m.load_state(&first).expect("our own state loads");
        give(&mut c, a as u8);
        sweep_play(&mut c, false, "ability");
        c.tick();
        a += c.of;
    }
    c.cov.phase = "screen";
    let english = c.m.bus.rom;
    let crc = c.m.bus.rom_crc();
    let mut copies = Vec::new();
    let mut i = c.index;
    // The screens in each language, then the pause menu's help with each
    // ability in each language (`pause_help.c`: its text is
    // `[language][2 + ability]` of the tiled backgrounds). Language is the
    // fastest index, so with a multiple of six workers each plays one.
    let screens = SCREENS.len() as u32 * LANGUAGES as u32;
    let items = screens + ABILITIES * LANGUAGES as u32;
    while i < items && c.cov.start.elapsed().as_secs() < secs {
        let (item, lang) = (i / LANGUAGES as u32, (i % LANGUAGES as u32) as u16);
        c.m.load_state(&first).expect("our own state loads");
        c.m.bus.rom = rom_in(&mut copies, english, lang);
        c.m.bus.keep_rom_crc(crc);
        refill(&mut c);
        match SCREENS.get(item as usize) {
            Some(s) => screen_play(&mut c, s),
            None => {
                give(&mut c, (item - SCREENS.len() as u32) as u8);
                screen_play(&mut c, &HELP);
            }
        }
        c.m.bus.rom = english;
        c.m.bus.keep_rom_crc(crc);
        c.tick();
        i += c.of;
    }
    c.cov.phase = "fuzz";
    while c.cov.start.elapsed().as_secs() < secs {
        c.tick();
        let id = c.load_pick();
        let why = if c.rng.chance(15) {
            poke(&mut c);
            "poke"
        } else {
            "play"
        };
        c.random_run(id, why);
    }
    c.stat();
    Ok(())
}
