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
/// first room; the frames it plays and how. `pokes` (address, value,
/// bytes) are written after the fade, before the creator is called (so a
/// screen with pokes is always called after the fade). `task`, if given,
/// puts a task of the screen in one of its states once it's up (pokes
/// written again first).
struct Screen {
    name: &'static str,
    create: u32,
    arg: Option<u32>,
    frames: u64,
    keys: Keys,
    pokes: &'static [(u32, u32, u8)],
    task: Option<TaskState>,
}

/// A screen's task put in a state: the task whose `main` is `main`, its
/// state function pointer at `at` in its struct set to `state`, `wait`
/// frames after the screen's creator.
#[derive(Clone, Copy)]
struct TaskState {
    main: u32,
    at: u32,
    state: u32,
    wait: u64,
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
        pokes: &[],
        task: None,
    }
}

impl Screen {
    /// The same screen with `pokes` written before its creator.
    const fn with(self, pokes: &'static [(u32, u32, u8)]) -> Screen {
        Screen { pokes, ..self }
    }

    /// The same screen with its task `main` put in `state` (its pointer
    /// at `at`) `wait` frames in.
    const fn state(self, main: u32, at: u32, state: u32, wait: u64) -> Screen {
        Screen {
            task: Some(TaskState {
                main,
                at,
                state,
                wait,
            }),
            ..self
        }
    }

    /// The file select's menu, put in state function `state`.
    const fn menu(name: &'static str, state: u32) -> Screen {
        screen_item(name, FILE_SELECT, None, 1200, Keys::Mash).state(
            MENU_TASK,
            MENU_STATE,
            state,
            MENU_FRAMES,
        )
    }
}

/// `sub_08138B44`: the file select and its menu, as the title opens it.
const FILE_SELECT: u32 = 0x0813_8B45;
/// The main menu's task (`sub_08141E74`, `code_08138D64.c`): its state
/// is a function pointer at 0x9CC of its struct.
const MENU_TASK: u32 = 0x0814_1E75;
const MENU_STATE: u32 = 0x9CC;
/// Frames the menu takes to come up before its state is set.
const MENU_FRAMES: u64 = 240;
/// `gTasks` (`MAX_TASK_NUM`, 0x14 bytes each: `structOffset` at 6, `main`
/// at 8, `flags` at 0x12, `TASK_USE_EWRAM` 0x10).
const TASKS: (u32, u32) = (0x0300_19F0, 0x80);
/// Link and multiplayer globals (`include/data.h`).
const LINK_MODE: u32 = 0x0203_8580;
const LOCAL_PLAYER: u32 = 0x0203_AD3C;
const HOST_PLAYER: u32 = 0x0203_AD24;
/// The multiplayer sub-game menu for a player who isn't the host
/// (`sub_0801E6C4(n)`; its `sub_0801F4BC` draws `gUnk_082DE8DC[n]`
/// when `gLocalPlayerId != 0`).
const SUBGAME_GUEST: u32 = 0x0801_E6C5;
/// `sub_0801A744(n)`, a sub-game's menu for one player (as SCREENS'
/// "sub-game n"), its task (`sub_0801FD58`, state at 0x154) put in
/// `sub_0801F8EC`, which draws `gUnk_082DE96C[n]`.
const SUBGAME: u32 = 0x0801_A745;
const SUBGAME_TASK: (u32, u32, u32) = (0x0801_FD59, 0x154, 0x0801_F8ED);
const GUEST: &[(u32, u32, u8)] = &[(LOCAL_PLAYER, 1, 1), (HOST_PLAYER, 0, 1)];
/// `sub_0801E754(n)`: the screen a sub-game shows while it sends itself
/// to the other players (multi_08019F28.c's `sub_0801A884`).
const SUBGAME_SEND: u32 = 0x0801_E755;
/// The game finished at 100% (`code_0814EBE4.c`, `sub_0814EBE4`, from the
/// ending): its task (`sub_0814EF9C`, state at 4) first recounts the
/// file's completion (`sub_08141E30`), so it's put in the 100% state
/// (`sub_0814ED78`) directly.
const HUNDRED: (u32, u32, u32, u32) = (0x0814_EBE5, 0x0814_EF9D, 4, 0x0814_ED79);
/// The link screen (`sub_08032E98`): its task (`sub_080331E0`, state at
/// 0x40) put in `sub_080330A4`, which draws `gUnk_082EC784` if
/// `gUnk_02038580 == 1` (the link code sets it on its own, so it's
/// written again just before).
const LINK: (u32, u32, u32, u32) = (0x0803_2E99, 0x0803_31E1, 0x40, 0x0803_30A5);

/// The screens, from the katam decomp's callers of the fade and of the
/// task creators (`demo.c`, `title_screen.c`, `code_08138D64.c`,
/// `multi_08019F28.c`, `dark_mind.c`, `kirby.c`, `multi_08032E98.c`),
/// then (assets/kirby-fuzz-last-misses) the ones the first sweep missed:
/// the menu's link states, the link screen's other mode, the multiplayer
/// sub-game screens, the 100% screen and the mirror's map.
const SCREENS: [Screen; 35] = [
    screen_item("logo", 0x0813_87B1, None, 1200, Keys::Still),
    screen_item("title", 0x0814_9CE5, None, 1800, Keys::Mash),
    // `CreateCutscene`: the intro, the mirror broken.
    screen_item("intro", 0x0802_6045, None, 3600, Keys::Still),
    // `sub_08138B44`: the file select and its menu, as the title opens it.
    screen_item("file select", FILE_SELECT, None, 1800, Keys::Mash),
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
    // The menu's states that load `gUnk_08385C14[gLanguage][n]` for the
    // columns the masher reaches by luck or never (3, 4, 6, 13; 14 and 15
    // only on a link).
    Screen::menu("menu col 3", 0x0813_D551),
    Screen::menu("menu col 4", 0x0813_D785),
    Screen::menu("menu col 6", 0x0813_D365),
    Screen::menu("menu col 13", 0x0813_ECBD),
    Screen::menu("menu col 14", 0x0814_04AD),
    Screen::menu("menu col 15", 0x0813_FF6D),
    // The link screen in its other mode (`gUnk_02038580 == 1`:
    // `gUnk_082EC784`).
    screen_item("link, mode 1", LINK.0, None, 1200, Keys::Mash)
        .with(&[(LINK_MODE, 1, 4)])
        .state(LINK.1, LINK.2, LINK.3, 10),
    screen_item("sub-game guest 0", SUBGAME_GUEST, Some(0), 1800, Keys::Mash).with(GUEST),
    screen_item("sub-game guest 1", SUBGAME_GUEST, Some(1), 1800, Keys::Mash).with(GUEST),
    screen_item("sub-game guest 2", SUBGAME_GUEST, Some(2), 1800, Keys::Mash).with(GUEST),
    screen_item("sub-game send 0", SUBGAME_SEND, Some(0), 600, Keys::Still),
    screen_item("sub-game send 1", SUBGAME_SEND, Some(1), 600, Keys::Still),
    screen_item("sub-game send 2", SUBGAME_SEND, Some(2), 600, Keys::Still),
    screen_item("100%", HUNDRED.0, None, 1800, Keys::Still)
        .state(HUNDRED.1, HUNDRED.2, HUNDRED.3, 0),
    screen_item("sub-game 0 menu", SUBGAME, Some(0), 600, Keys::Still).state(
        SUBGAME_TASK.0,
        SUBGAME_TASK.1,
        SUBGAME_TASK.2,
        60,
    ),
    screen_item("sub-game 1 menu", SUBGAME, Some(1), 600, Keys::Still).state(
        SUBGAME_TASK.0,
        SUBGAME_TASK.1,
        SUBGAME_TASK.2,
        60,
    ),
    screen_item("sub-game 2 menu", SUBGAME, Some(2), 600, Keys::Still).state(
        SUBGAME_TASK.0,
        SUBGAME_TASK.1,
        SUBGAME_TASK.2,
        60,
    ),
    // `sub_0801DFE8`: `sub_0801DA58(0)`, the map of the mirror's world
    // (`gUnk_082D7850[0x307]` and `[0x308]`), from the pause map.
    screen_item("mirror map", 0x0801_DFE9, None, 1200, Keys::Mash),
];

/// The pause menu's help slots (`sHelpMenuUnkTiledBGsIndices[language]
/// [2 + ability]`): every ability, and three past the last that no
/// ability reaches.
const HELP_SLOTS: u32 = ABILITIES + 3;

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

/// The struct of task `ts.main`, if it's up: a task with that `main`
/// whose state pointer is a Thumb function in ROM (a destroyed task's
/// slot keeps its `main`).
fn find_task(c: &Core, ts: &TaskState) -> Option<u32> {
    (0..TASKS.1).find_map(|i| {
        let t = TASKS.0 + i * 0x14;
        if c.m.bus.read32(t + 8) != ts.main {
            return None;
        }
        let off = u32::from(c.m.bus.read16(t + 6));
        let at = if c.m.bus.read16(t + 0x12) & 0x10 != 0 {
            0x0200_0000 + (off << 2)
        } else {
            0x0300_0000 + off
        };
        let state = c.m.bus.read32(at + ts.at);
        ((0x0800_0000..0x0A00_0000).contains(&state) && state & 1 == 1).then_some(at)
    })
}

/// Write `value`'s low `bytes` bytes at `at`.
fn poke_bytes(c: &mut Core, at: u32, value: u32, bytes: u8) {
    match bytes {
        1 => c.m.bus.write8(at, value as u8),
        2 => c.m.bus.write16(at, value as u16),
        _ => c.m.bus.write32(at, value),
    }
}

/// Play screen item `s` from the first room, in the ROM now on the bus.
/// `late`, if given, is an ability written on Kirby once the pause menu
/// is up (abilities past the last, which his own code can't run).
fn screen_play(c: &mut Core, s: &Screen, late: Option<u8>) {
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
        let after = s.arg.is_some() || !s.pokes.is_empty() || s.task.is_some();
        let callback = if after { NOP } else { s.create };
        if !core::call(&mut c.m, FADE_TO, &[callback]) {
            c.faults += 1;
            return;
        }
        if after {
            if c.play(FADE_FRAMES, 0, &why, |_, _| 0, &[]).is_none() {
                return;
            }
            frames -= FADE_FRAMES;
            for &(at, value, bytes) in s.pokes {
                poke_bytes(c, at, value, bytes);
            }
            if !core::call(&mut c.m, s.create, &[s.arg.unwrap_or(0)]) {
                c.faults += 1;
                return;
            }
        }
    }
    if let Some(ts) = s.task {
        if c.play(ts.wait, 0, &why, |_, _| 0, &[]).is_none() {
            return;
        }
        frames -= ts.wait;
        let Some(task) = find_task(c, &ts) else {
            c.faults += 1;
            return;
        };
        for &(at, value, bytes) in s.pokes {
            poke_bytes(c, at, value, bytes);
        }
        c.m.bus.write32(task + ts.at, ts.state);
    }
    // Frames already played of the presses below.
    let mut skip = 0;
    if let Some(a) = late {
        // The pause menu is up 14 frames in (START at 10).
        skip = 14;
        if c.play(
            skip,
            0,
            &why,
            |f, _| if f >= 10 { key::START } else { 0 },
            &[],
        )
        .is_none()
        {
            return;
        }
        frames -= skip;
        c.m.bus.write8(KIRBY + ABILITY, a);
    }
    c.sweeps += 1;
    let keys = s.keys;
    c.play(
        frames,
        0,
        &why,
        |f, r| {
            let f = f + skip;
            match keys {
                Keys::Still => 0,
                Keys::Browse if f < 60 || f % 20 > 1 => 0,
                Keys::Browse if (f / 20) % 7 == 6 => key::A,
                Keys::Browse if (f / 20) % 5 == 4 => key::DOWN,
                Keys::Browse => key::RIGHT,
                Keys::Pause if f < 10 => 0,
                Keys::Pause if f < 14 => key::START,
                Keys::Pause if f < 120 => 0,
                _ => p.keys(r),
            }
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
    // help slot in each language (`pause_help.c`: its text is
    // `[language][2 + ability]` of the tiled backgrounds, HELP_SLOTS of
    // them). Language is the
    // fastest index, so with a multiple of six workers each plays one.
    let screens = SCREENS.len() as u32 * LANGUAGES as u32;
    let items = screens + HELP_SLOTS * LANGUAGES as u32;
    while i < items && c.cov.start.elapsed().as_secs() < secs {
        let (item, lang) = (i / LANGUAGES as u32, (i % LANGUAGES as u32) as u16);
        c.m.load_state(&first).expect("our own state loads");
        c.m.bus.rom = rom_in(&mut copies, english, lang);
        c.m.bus.keep_rom_crc(crc);
        refill(&mut c);
        match SCREENS.get(item as usize) {
            Some(s) => screen_play(&mut c, s, None),
            None => {
                let a = (item - SCREENS.len() as u32) as u8;
                if u32::from(a) < ABILITIES {
                    give(&mut c, a);
                    screen_play(&mut c, &HELP, None);
                } else {
                    screen_play(&mut c, &HELP, Some(a));
                }
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
