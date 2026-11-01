//! Emerald's fuzzing worker: a process with one machine, since `runtime::prov`'s
//! tags and bitmaps are process-wide. It plays the new-game route, boots
//! the crafted save, then loops: pick a kept state, maybe mutate the game
//! in RAM, play random presses from it, and keep the state again when the
//! run reached something new. Now and then it boots a mutated save
//! instead. Every first sighting goes to `events.tsv`. What isn't
//! Emerald's (the frame loop, coverage, corpus, presses, logs) is in
//! src/core.rs (assets/kirby-fuzz).
//!
//! v2 (assets/fuzz-v2): *sweeps* walk lists a random draw reaches slowly,
//! each worker taking every `of`-th item from its `index`: a wild battle
//! against each species (normal, then shiny; the party's lead the same, for
//! its back picture), a battle against each trainer, and a boot of each map
//! with the bag's pockets full, the Pokédex complete and the system flags
//! on. Workers share a corpus through `OUT/shared/`: states that found much,
//! and their coverage bitmaps, so "new" means new to all of them.
//!
//! assets/fuzz-dex-bag-sweeps: before fuzzing, each worker pages through
//! its share of the national Pokédex's entries (the only screen that draws
//! footprints) and scrolls its share of the bag passes (every item, its
//! icon drawn as the cursor lands on it), opened from the start menu with
//! the game's state poked in RAM. Footprints are expanded from 1bpp in
//! code, so the worker logs `table` rows too: computed bytes that reached
//! the screen, with the ROM table the provenance tags guess they came from.
//!
//! assets/fuzz-move-sweep: after the PC passes, each worker uses its share of
//! the moves, one wild battle each: the party's lead knowing only that
//! move, against a mon it can't miss for type, the battle scene on, A
//! pressed every 8 frames (FIGHT, then the first move). A move's
//! animation, and the graphics it loads, plays only when it's used.
//! `scenes.tsv` has a `move-N` row for each. assets/fuzz-move-sweep-foe-side
//! then has the wild mon use each move on the lead (a `foe-move-N` row),
//! for the backgrounds drawn when the player's side is the target, and
//! pokes a stockpile for Spit Up and Swallow.
//!
//! assets/fuzz-story-seeds: given a folder of story-point saves
//! (`emerald-story`, `crates/emerald-save/story.toml`), each worker boots every
//! one after its sweeps and keeps the overworld state as a *story seed*:
//! never dropped, and one pick in [`STORY_PICK`] is a story seed's. What a
//! run finds is credited with graphics (a blob, copied or table run reaching
//! the screen) [`GFX_WEIGHT`] times an instruction or a load, so picks lean
//! toward the states whose runs load graphics not yet seen.
//!
//! assets/fuzz-tas-bios: the folder may hold mGBA states too
//! (`state-N.ss`, saved along a TAS by `mgba-dump --bk2`), each loaded,
//! run [`STATE_SETTLE`] frames and kept as a story seed. The cartridge's
//! save isn't in an mGBA state, so it stays whatever the worker last booted.
//!
//! assets/fuzz-contest-frontier-sweeps: after the story seeds, each worker
//! plays its share of the *scenes*, booted from the furthest story save
//! with a party of six at level 50 holding every master ribbon: each
//! contest (five categories at four ranks), entered by a script written to
//! free EWRAM that does what the lobby's receptionist does once the
//! choices are made, and each Battle Frontier challenge, its attendant's
//! script started. The game's script context is pointed at the script as
//! `ScriptContext_SetupScript` would; A every 8 frames takes each menu's
//! first choice from there, the party menu's three (or two, or four) mons
//! entered in RAM and the Battle Factory's rentals pressed their own way.
//! `scenes.tsv` has where each one got to. assets/fuzz-contest-appeal-anims:
//! the entered mon knows first one of the five moves whose appeal draws a
//! contest-only background ([`CONTEST_MOVES`]), so A appeals with it.
//!
//! assets/fuzz-pc-box-sweep: after the bag passes, each worker plays its
//! share of the *PC passes*: the 14 boxes filled in RAM with every species
//! and Unown's 27 other forms (each its own icon), the PC's menu opened by
//! its special from a script, and each box walked slot by slot (the
//! hovered mon's picture and data), on its own wallpaper, then the box
//! menu and the party; once in move items mode (each mon holding an item,
//! the first's Info shown) and once on Walda's wallpaper, its pattern and
//! icon poked before each scroll. Rows in `scenes.tsv` too. `--pc-only 1`
//! stops a worker there.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use emerald_save as es;
use runtime::Machine;
use runtime::io::key;

use crate::core::{Core, Pad, Presses, ROM, RUN_MAX, WINDOW};
use crate::rng::Rng;
/// `gMain.callback2` (`gMain` at 0x030022C0): the screen the game is on.
const CALLBACK2: u32 = 0x0300_22C4;
/// `gSaveBlock2Ptr`.
const SB2_PTR: u32 = 0x0300_5D90;
/// `gMain.savedCallback`, `gMain.state`.
const SAVED_CALLBACK: u32 = 0x0300_22C8;
const MAIN_STATE: u32 = 0x0300_26F8;
/// `gPlayerPartyCount`, `gPlayerParty`, `gEnemyParty`.
const PARTY_COUNT: u32 = 0x0202_44E9;
const PARTY: u32 = 0x0202_44EC;
const ENEMY_PARTY: u32 = 0x0202_4744;
/// `gBattleTypeFlags` (`BATTLE_TYPE_TRAINER` is 8), `gTrainerBattleOpponent_A`.
const BATTLE_TYPE: u32 = 0x0202_2FEC;
const TRAINER_OPPONENT: u32 = 0x0203_8BCA;
/// `CB2_InitBattle`, `CB2_ReturnToField` (Thumb).
const CB2_INIT_BATTLE: u32 = 0x0803_6761;
const CB2_RETURN_TO_FIELD: u32 = 0x0808_60C9;
/// `sStartMenuCursorPos`, `sLastSelectedPokemon` (the Pokédex list's
/// cursor when it opens), `gBagPosition` (its `pocket` at 5, then each
/// pocket's `cursorPosition` and `scrollPosition`).
const START_MENU_CURSOR: u32 = 0x0203_760E;
const DEX_LAST_SELECTED: u32 = 0x0203_9B50;
/// `sPokedexListItem`: the entry page's entry (its `dexNum` first), set
/// as the page loads; 0 until the Pokédex first opens one.
const DEX_LIST_ITEM: u32 = 0x0203_9B54;
const BAG_POSITION: u32 = 0x0203_CE58;
/// `SaveBlock2.pokedex`: `order`, `mode` (1 national), `nationalMagic`.
const SB2_DEX: u32 = 0x18;
/// `FLAG_SYS_POKEMON_GET`, `FLAG_SYS_POKEDEX_GET`, `FLAG_SYS_NATIONAL_DEX`,
/// and `VAR_NATIONAL_DEX` (0x4046) as an index into `vars`: with
/// `nationalMagic` 0xDA and the var 0x302, `IsNationalPokedexEnabled`.
const FLAGS_DEX: [u32; 3] = [0x860, 0x861, 0x896];
const VAR_NATIONAL_DEX: u32 = 0x46;
/// `NATIONAL_DEX_COUNT`, and the entries a dex sweep step pages through.
const DEX_ENTRIES: u32 = 386;
const DEX_CHUNK: u32 = 32;
/// Frames between presses in the Pokédex. The entry page takes a Down only
/// once the last one's entry has loaded (about 100 to 160 frames), so none
/// is skipped; the sweep reads which entry is up after each step.
const DEX_STEP: u64 = 20;
/// Item ids: TMs and HMs (`ITEM_TM01` to `ITEM_HM08`) and berries
/// (`ITEM_CHERI_BERRY` to `ITEM_ENIGMA_BERRY`) go in their own pockets,
/// whose lists print a move or a berry number; every other item in the
/// items, Poké Balls and key items pockets.
const TMS: std::ops::RangeInclusive<u32> = 289..=346;
const BERRIES: std::ops::RangeInclusive<u32> = 133..=175;
/// Bag passes: the general items, 76 at a time.
const BAG_PASSES: u32 = 4;
/// Frames between Downs in the bag's lists.
const BAG_STEP: u64 = 8;
/// The PC sweep (assets/fuzz-pc-box-sweep): `gPokemonStoragePtr`, whose
/// `PokemonStorage` holds `currentBox`, then 14 boxes of 30 `BoxPokemon`
/// (a `struct Pokemon`'s first 80 bytes) from 4, their names, then each
/// box's wallpaper.
const STORAGE_PTR: u32 = 0x0300_5D94;
const STORAGE_BOXES: u32 = 4;
const STORAGE_WALLPAPERS: u32 = 0x83C2;
const BOXES: u32 = 14;
const BOX_SLOTS: u32 = 30;
const BOX_COLUMNS: u32 = 6;
const BOX_MON: usize = 80;
/// `WALLPAPER_COUNT`: 16, then `WALLPAPER_FRIENDS` (Walda's, its pattern
/// `SaveBlock1.waldaPhrase.patternId`).
const WALLPAPERS: u32 = 17;
const SB1_WALDA_ICON: u32 = 0x3D70 + 20;
const SB1_WALDA_PATTERN: u32 = 0x3D70 + 21;
/// Walda's wallpaper patterns (`sWaldaWallpapers`) and icons
/// (`sWaldaWallpaperIcons`), and the box scrolls that show them all.
const WALDA_PATTERNS: u32 = 16;
const WALDA_ICONS: u32 = 30;
const WALDA_SCROLLS: u32 = 30;
/// `SPECIES_UNOWN`, and its forms past A (B to Z, ! and ?), each its own
/// icon, picked by the personality.
const UNOWN: u16 = 201;
const UNOWN_FORMS: u32 = 27;
/// `ShowPokemonStorageSystemPC`'s index in `gSpecials`: the PC's menu
/// (withdraw, deposit, move Pokémon, move items, see ya).
const SPECIAL_SHOW_PC: u16 = 63;
/// The PC passes: which 14 boxes' worth of the list (every species, then
/// Unown's other forms), the PC's option (2 move Pokémon, 3 move items,
/// each mon holding an item) and whether it's the Walda pass.
const PC_PASSES: [(u32, u8, bool); 4] = [(0, 2, false), (1, 2, false), (0, 3, false), (0, 2, true)];
/// Frames between presses in a box: the cursor's move and the hovered
/// mon's picture and data drawn.
const PC_STEP: u64 = 16;
/// `TRAINERS_COUNT`.
const TRAINERS: u32 = 855;
/// `gMapGroups`: a pointer a group, each to its maps' `MapHeader`s, whose
/// `mapLayoutId` is at 0x12.
const MAP_GROUPS: u32 = 0x0848_6578;
/// Maps in each group (`include/constants/map_groups.h`).
const MAP_COUNTS: [u8; 34] = [
    57, 5, 5, 6, 7, 8, 9, 7, 7, 14, 8, 17, 10, 23, 13, 15, 15, 2, 2, 2, 3, 1, 1, 1, 108, 61, 89, 2,
    1, 13, 1, 1, 3, 1,
];
/// The move sweep (assets/fuzz-move-sweep): the wild mon each move is used
/// on, `SPECIES_SQUIRTLE` (pure Water, so no move's type misses it, and
/// Torrent, an ability that stops none), knowing only `MOVE_SPLASH`, so it
/// ends no battle; both at level 100, unless [`MoveSetup`] says otherwise
/// for the move. Frames a move's battle plays: the
/// intro, then the move chosen and used about three times.
const MOVE_TARGET: u16 = 7;
const MOVE_SPLASH: u16 = 150;
const MOVE_LEVEL: u8 = 100;
const MOVE_FRAMES: u64 = 1800;
/// `MOVE_SPIT_UP` and `MOVE_SWALLOW`, which fail (unanimated) unless the
/// user has stockpiled; `gDisableStructs[0].stockpileCounter` (each
/// battler's `DisableStruct` 0x1C bytes on), poked to 3 every
/// `STOCKPILE_EVERY` frames once `BattleMainCB2` runs (a switch-in
/// clears it, Spit Up empties it). A multiple of [`WINDOW`].
const MOVE_SPIT_UP: u32 = 255;
const MOVE_SWALLOW: u32 = 256;
const MOVE_CURSE: u32 = 174;
const STOCKPILE: u32 = 0x0202_42C5;
const DISABLE_STRUCT: u32 = 0x1C;
const STOCKPILE_EVERY: u64 = 60;
/// `BattleMainCB2`, `gMain.callback2` once the battle is set up.
const CB2_BATTLE: u32 = 0x0803_8421;
/// `SaveBlock2`'s option bits at 0x14: text speed in bits 0-2 (2 fast),
/// `optionsBattleSceneOff` bit 10; without the scene no move animates.
const SB2_OPTIONS: u32 = 0x14;
/// `FLAG_BADGE08_GET`: the lead's trainer isn't the player (its OT is
/// FUZZ), so at level 100 it obeys only with the eighth badge; without
/// it, it ignores orders and falls asleep, and the move never plays.
const FLAG_BADGE08: u32 = 0x86E;
/// `NUM_SPECIES`, `MOVES_COUNT`, `ITEMS_COUNT` (pokeemerald's constants).
const SPECIES: u32 = 412;
const MOVES: u32 = 355;
const ITEMS: u32 = 377;
/// The new-game route's last frame (the truck), and the frames whose
/// states it keeps.
const ROUTE_END: u64 = 5400;
const ROUTE_KEEP: [u64; 5] = [900, 1000, 2000, 3800, 5400];
/// Continue from a save: Start, A at the main menu, A again; the
/// overworld is up by this frame.
const CONTINUE: &str = "start@300,a@900,a@1100";
const CONTINUE_END: u64 = 1400;
/// Frames run, unpressed, from a TAS's state before it's kept.
const STATE_SETTLE: u64 = 60;
/// The scene sweep (assets/fuzz-contest-frontier-sweeps):
/// `MAP_LILYCOVE_CITY_CONTEST_LOBBY` and where the player stands in it;
/// each Battle Frontier lobby (`MAP_BATTLE_FRONTIER_*_LOBBY`, group 26)
/// with an attendant, its tile (each `map.json`) and its script
/// (`BattleFrontier_*Lobby_EventScript_*Attendant`), the player standing
/// across the counter from it.
const CONTEST_LOBBY: es::Map = es::Map { group: 13, num: 4 };
const CONTEST_AT: (i16, i16) = (14, 4);
const FRONTIER_GROUP: u8 = 26;
const FRONTIER: [(&str, u8, (i16, i16), u32); 12] = [
    ("tower-singles", 5, (6, 5), 0x0823_E936),
    ("tower-doubles", 5, (10, 5), 0x0823_EA9F),
    ("tower-multis", 5, (14, 5), 0x0823_EC08),
    ("dome-singles", 18, (5, 10), 0x0824_9A35),
    ("dome-doubles", 18, (17, 10), 0x0824_9A47),
    ("palace-singles", 22, (5, 6), 0x0824_D944),
    ("palace-doubles", 22, (19, 6), 0x0824_D956),
    ("arena", 28, (7, 7), 0x0825_5DF4),
    ("factory-singles", 31, (4, 7), 0x0825_85CB),
    ("factory-doubles", 31, (14, 7), 0x0825_85DD),
    ("pike", 34, (5, 5), 0x0825_B868),
    ("pyramid", 25, (7, 12), 0x0825_08B1),
];
/// `CONTEST_CATEGORIES_COUNT` and the ranks (normal to master).
const CONTEST_CATEGORIES: u32 = 5;
const CONTEST_RANKS: u32 = 4;
/// Frames a scene plays: a contest (its hall, four rounds of appeals and
/// the results), a Frontier challenge.
const CONTEST_FRAMES: u64 = 30_000;
/// Moves whose appeal draws a contest-only background
/// (assets/fuzz-contest-appeal-anims): Mega Punch (`impact_contests`),
/// Horn Drill (`drill_contests`), Guillotine (`guillotine_contests`),
/// Scary Face (`scary_face_contest`), Surf (`water_contest`). Contest `i`'s
/// entered mon knows `CONTEST_MOVES[i % 5]` first, the move A picks each
/// round (the cursor stays where it was).
const CONTEST_MOVES: [u16; 5] = [5, 32, 12, 184, 57];
const FRONTIER_FRAMES: u64 = 36_000;
/// Each this many of a scene's frames is a row of `scenes.tsv`.
const SCENE_CHUNK: u64 = 600;
/// Screens a scene presses its own way on (`gMain.callback2`): the party
/// menu, in battle (Down, A, A sends out the next mon, the one up having
/// fainted) or choosing a Frontier challenge's mons into
/// `gSelectedOrderFromParty` (as many as `gSpecialVar_0x8005`); the
/// summary screen (B leaves it, or A on its first page); the Dome's
/// tourney tree (a trainer's card, then back); the Battle Factory's rental and swap
/// screens and the overworld past a Frontier lobby (random presses: the
/// Pike's paths and the Pyramid's floors are walked, and the Dome's menus
/// have more than their first choice).
const CB2_UPDATE_PARTY_MENU: u32 = 0x081B_01B1;
/// `gPartyMenu.menuType` (its low 4 bits; 1 is `PARTY_MENU_TYPE_IN_BATTLE`).
const PARTY_MENU_TYPE: u32 = 0x0203_CED0;
const SELECTED_ORDER: u32 = 0x0203_CEF8;
const SPECIAL_VAR_8005: u32 = 0x0203_75E2;
const CB2_INIT_SUMMARY_SCREEN: u32 = 0x081B_FAE5;
const CB2_SUMMARY_SCREEN: u32 = 0x081B_FAB5;
const CB2_FACTORY_SELECT_SCREEN: u32 = 0x0819_A4C9;
const CB2_FACTORY_SWAP_SCREEN: u32 = 0x0819_C7E1;
const CB2_TOURNEY_TREE: u32 = 0x0819_4B55;
/// `CB2_Overworld`.
const CB2_OVERWORLD: u32 = 0x0808_5E5D;
/// A ribbon word with the master rank of every category (3 bits each,
/// cool first), so a mon may enter any contest.
const MASTER_RIBBONS: u32 = 0x4924;
/// The script engine: `sGlobalScriptContext` (`mode` at 1, `scriptPtr` at
/// 8), `sGlobalScriptContextStatus` (0 running), `sLockFieldControls`.
const SCRIPT_CONTEXT: u32 = 0x0300_0E40;
const SCRIPT_STATUS: u32 = 0x0300_0E38;
const LOCK_FIELD_CONTROLS: u32 = 0x0300_0F2C;
/// Free EWRAM past the last of pret's variables (`sRayScene`, 0x0203CF60),
/// where a scene's script is written.
const SCRIPT_RAM: u32 = 0x0203_F000;
/// `gContestMonPartyIndex`: the party slot entered.
const CONTEST_MON_INDEX: u32 = 0x0203_9F24;
/// The contest lobby's scripts that enter a contest once rank, category
/// and mon are chosen (`LilycoveCity_ContestLobby_EventScript_*` in
/// `data/maps/LilycoveCity_ContestLobby/scripts.inc` and
/// `data/maps/ContestHall/scripts.inc`), and the specials they call by
/// index in `gSpecials`.
const SCR_SET_PLAYER_GFX: u32 = 0x0823_BEB6;
const SCR_SET_CONTEST_TYPE: u32 = 0x0821_A585;
const SCR_WARP_TO_CONTEST_HALL: u32 = 0x0821_A5EF;
const SPECIAL_CLEAR_LINK_CONTEST_FLAGS: u16 = 444;
const SPECIAL_TRY_ENTER_CONTEST_MON: u16 = 87;
const SPECIAL_SET_CONTEST_TRAINER_GFX_IDS: u16 = 86;
/// `VAR_CONTEST_RANK`, `VAR_CONTEST_CATEGORY`, `VAR_CONTEST_HALL_STATE`.
const VAR_CONTEST_RANK: u16 = 0x8010;
const VAR_CONTEST_CATEGORY: u16 = 0x8011;
const VAR_CONTEST_HALL_STATE: u16 = 0x4086;
/// Script opcodes (`data/script_cmd_table.inc`).
const SCR_END: u8 = 0x02;
const SCR_CALL: u8 = 0x04;
const SCR_GOTO: u8 = 0x05;
const SCR_SETVAR: u8 = 0x16;
const SCR_SPECIAL: u8 = 0x25;
const SCR_WAITSTATE: u8 = 0x27;

/// The Emerald worker: the shared core (its machine, coverage and corpus,
/// reached through `Deref`) and what only Emerald has.
struct Worker {
    c: Core,
    rom: &'static [u8],
    base: es::Blocks,
    /// The furthest story point's blocks (or `base`), where scenes start.
    story_base: es::Blocks,
    scenes: std::io::BufWriter<std::fs::File>,
    /// The crafted save's overworld, where battles start.
    overworld: Vec<u8>,
    /// How far each sweep has gone.
    swept: [u32; 3],
}

impl std::ops::Deref for Worker {
    type Target = Core;
    fn deref(&self) -> &Core {
        &self.c
    }
}

impl std::ops::DerefMut for Worker {
    fn deref_mut(&mut self) -> &mut Core {
        &mut self.c
    }
}

/// What a move needs to work, so its animation plays: the lead's species
/// (Curse animates its Ghost form only for a Ghost), a gender apart from
/// the foe (Attract: the lead female, its personality's low byte 0, the
/// Squirtle male at 0xFF), who sleeps (Snore and Sleep Talk work only
/// asleep, Dream Eater and Nightmare only on a sleeping foe), the foe's
/// move (Counter and Mirror Coat return a hit, physical or special) and
/// level (a one-hit KO hits 30 plus the level gap in 100).
struct MoveSetup {
    lead: Option<u16>,
    gender_mask: u32,
    lead_asleep: bool,
    foe_asleep: bool,
    foe_move: u16,
    foe_level: u8,
}

/// Turns asleep, the low bits of a party mon's status.
const SLEEP_TURNS: u8 = 5;

impl MoveSetup {
    fn of(mv: u16) -> MoveSetup {
        // pokeemerald's MOVE_*, SPECIES_GASTLY and SPECIES_CLEFAIRY.
        let (gastly, clefairy, tackle, water_gun) = (92, 35, 33, 55);
        MoveSetup {
            lead: match mv {
                174 => Some(gastly),
                213 => Some(clefairy),
                _ => None,
            },
            gender_mask: if mv == 213 { 0xFF } else { 0 },
            lead_asleep: matches!(mv, 173 | 214),
            foe_asleep: matches!(mv, 138 | 171),
            foe_move: match mv {
                68 => tackle,
                243 => water_gun,
                _ => MOVE_SPLASH,
            },
            foe_level: if matches!(mv, 12 | 32 | 90 | 329) {
                MOVE_LEVEL - 70
            } else {
                MOVE_LEVEL
            },
        }
    }
}

/// The screen: `gMain.callback2`.
fn screen(m: &Machine<Pad>) -> u32 {
    m.bus.read32(CALLBACK2)
}

/// The map the player stands in, `group.num` (`255.255` before the save
/// blocks are set up).
fn place(m: &Machine<Pad>) -> String {
    let sb1 = m.bus.read32(es::SB1_PTR);
    if !(0x0200_0000..0x0204_0000).contains(&sb1) {
        return "255.255".into();
    }
    format!("{}.{}", m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5))
}

impl Worker {
    /// The map the player stands in, `(0xFF, 0xFF)` before the save
    /// blocks are set up.
    fn location(&self) -> (u8, u8) {
        let sb1 = self.m.bus.read32(es::SB1_PTR);
        if !(0x0200_0000..0x0204_0000).contains(&sb1) {
            return (0xFF, 0xFF);
        }
        (self.m.bus.read8(sb1 + 4), self.m.bus.read8(sb1 + 5))
    }

    /// The next item of sweep `k` (`n` items) for this worker, or `None`
    /// when it has done its share.
    fn next_item(&mut self, k: usize, n: u32) -> Option<u32> {
        let i = self.index + self.of * self.swept[k];
        if i >= n {
            return None;
        }
        self.swept[k] += 1;
        Some(i)
    }

    /// A sweep's battle from the overworld: wild against species `s`
    /// (shiny for the list's second half), the party's lead the same; or
    /// trainer `t`. Started as the game's own transition ends one:
    /// `gEnemyParty`, `gBattleTypeFlags` and the opponent set, then
    /// `CB2_InitBattle` with the field to return to.
    fn battle(&mut self, wild: Option<u32>, trainer: Option<u32>) {
        let state = self.overworld.clone();
        self.m.load_state(&state).expect("our own state loads");
        let sb2 = self.m.bus.read32(SB2_PTR);
        let ot = self.m.bus.read32(sb2 + 0x0A);
        if let Some(i) = wild {
            let (s, shiny) = (1 + i % (SPECIES - 1), i >= SPECIES - 1);
            let level = 5 + self.rng.below(60) as u8;
            let lo = self.rng.next() & 0xFFFF;
            let hi = (ot >> 16 ^ ot ^ lo) & 0xFFFF ^ if shiny { 0 } else { 0x100 };
            let mon = self.mon_of(s as u16, level, hi << 16 | lo, ot);
            for (at, n) in [(ENEMY_PARTY, None), (PARTY, Some(PARTY_COUNT))] {
                for (j, &x) in mon.iter().enumerate() {
                    self.m.bus.write8(at + j as u32, x);
                }
                if let Some(c) = n
                    && self.m.bus.read8(c) == 0
                {
                    self.m.bus.write8(c, 1);
                }
            }
            self.m.bus.write32(BATTLE_TYPE, 0);
        }
        if let Some(t) = trainer {
            self.m.bus.write32(BATTLE_TYPE, 8);
            self.m.bus.write16(TRAINER_OPPONENT, t as u16);
        }
        self.m.bus.write32(SAVED_CALLBACK, CB2_RETURN_TO_FIELD);
        self.m.bus.write8(MAIN_STATE, 0);
        self.m.bus.write32(CALLBACK2, CB2_INIT_BATTLE);
        self.sweeps += 1;
        let mut p = Presses {
            walker: false,
            held: 0,
            left: 0,
            gap: 0,
        };
        let why = if wild.is_some() { "wild" } else { "trainer" };
        self.play(RUN_MAX, 0, why, |_, r| p.keys(r), &[]);
    }

    /// Move sweep step: a wild battle in which the party's lead (a random
    /// species, at three quarters of its HP) knows only move `mv`, against
    /// [`MOVE_TARGET`], set up as [`MoveSetup`] says. A every 8
    /// frames takes the intro's text, FIGHT and the first move each turn,
    /// so the move's animation plays, its graphics loaded. With
    /// `foe_side` the roles swap (assets/fuzz-move-sweep-foe-side): the
    /// wild mon knows `mv` and uses it on a lead [`MOVE_TARGET`], so the
    /// animation's opponent-side backgrounds (`*_Player` tilemaps, the
    /// player's side the target) draw. Spit Up and Swallow fail without a
    /// stockpile, so [`STOCKPILE`] is poked for both battlers each
    /// [`STOCKPILE_EVERY`] frames. A row of `scenes.tsv` says what it found.
    fn move_battle(&mut self, mv: u32, foe_side: bool) {
        let state = self.overworld.clone();
        self.m.load_state(&state).expect("our own state loads");
        let sb2 = self.m.bus.read32(SB2_PTR);
        let ot = self.m.bus.read32(sb2 + 0x0A);
        let o = self.m.bus.read16(sb2 + SB2_OPTIONS) & !(1 << 10 | 7) | 2;
        self.m.bus.write16(sb2 + SB2_OPTIONS, o);
        let sb1 = self.m.bus.read32(es::SB1_PTR);
        let a = sb1 + es::SB1_FLAGS as u32 + FLAG_BADGE08 / 8;
        let v = self.m.bus.read8(a) | 1 << (FLAG_BADGE08 % 8);
        self.m.bus.write8(a, v);
        let set = MoveSetup::of(mv as u16);
        // The foe's Curse is a Squirtle's, the stat form (its mask,
        // `AnimTask_DrawFallingWhiteLinesOnAttacker`), the lead's a Gastly's.
        let user = match set.lead {
            Some(_) if foe_side && mv == MOVE_CURSE => MOVE_TARGET,
            Some(s) => s,
            None => 1 + self.rng.below(SPECIES - 1) as u16,
        };
        let p = self.rng.next() & !set.gender_mask;
        let mut mon = Self::mon_with(user, MOVE_LEVEL, p, ot, [(mv as u16, 40), (0, 0)]);
        // At three quarters of its HP, so a move that heals has something
        // to heal (at full HP it fails, unanimated).
        let max = u16::from_le_bytes([mon[88], mon[89]]);
        mon[86..88].copy_from_slice(&(max / 4 * 3).to_le_bytes());
        if set.lead_asleep {
            mon[80] = SLEEP_TURNS;
        }
        let p = self.rng.next() | set.gender_mask;
        let mut target = Self::mon_with(
            MOVE_TARGET,
            set.foe_level,
            p,
            ot,
            [(set.foe_move, 40), (0, 0)],
        );
        if set.foe_asleep {
            target[80] = SLEEP_TURNS;
        }
        let (lead, foe) = if foe_side {
            (target, mon)
        } else {
            (mon, target)
        };
        for (j, &x) in lead.iter().enumerate() {
            self.m.bus.write8(PARTY + j as u32, x);
        }
        if self.m.bus.read8(PARTY_COUNT) == 0 {
            self.m.bus.write8(PARTY_COUNT, 1);
        }
        for (j, &x) in foe.iter().enumerate() {
            self.m.bus.write8(ENEMY_PARTY + j as u32, x);
        }
        self.m.bus.write32(BATTLE_TYPE, 0);
        self.m.bus.write32(SAVED_CALLBACK, CB2_RETURN_TO_FIELD);
        self.m.bus.write8(MAIN_STATE, 0);
        self.m.bus.write32(CALLBACK2, CB2_INIT_BATTLE);
        self.sweeps += 1;
        let name = if foe_side {
            format!("foe-move-{mv}")
        } else {
            format!("move-{mv}")
        };
        let stockpile = matches!(mv, MOVE_SPIT_UP | MOVE_SWALLOW);
        let mut found = Some(0);
        let mut f = 0;
        while f < MOVE_FRAMES
            && let Some(n) = found
        {
            if stockpile && self.m.bus.read32(CALLBACK2) == CB2_BATTLE {
                for b in 0..2 {
                    self.m.bus.write8(STOCKPILE + DISABLE_STRUCT * b, 3);
                }
            }
            let chunk = STOCKPILE_EVERY.min(MOVE_FRAMES - f);
            let at = f;
            found = self
                .play(
                    chunk,
                    0,
                    "move",
                    |g, _| if (at + g) % 8 < 2 { key::A } else { 0 },
                    &[],
                )
                .map(|k| n + k);
            f += chunk;
        }
        let cb2 = self.m.bus.read32(CALLBACK2);
        let (g, m) = self.location();
        let found = found.map_or("fault".into(), |n| n.to_string());
        let _ = writeln!(
            self.scenes,
            "{name}\t{MOVE_FRAMES}\t{cb2:#010x}\t{g}.{m}\t{found}"
        );
    }

    /// The overworld, with the Pokédex (national, every entry seen and
    /// owned) and the party in the start menu; its cursor on item `cursor`
    /// (0 the Pokédex, 2 the bag). `None` if the save blocks aren't there.
    fn menu_ready(&mut self, cursor: u8) -> Option<(u32, u32)> {
        let state = self.overworld.clone();
        self.m.load_state(&state).expect("our own state loads");
        let sb1 = self.m.bus.read32(es::SB1_PTR);
        let sb2 = self.m.bus.read32(SB2_PTR);
        if !(0x0200_0000..0x0204_0000).contains(&sb1) || !(0x0200_0000..0x0204_0000).contains(&sb2)
        {
            return None;
        }
        for f in FLAGS_DEX {
            let a = sb1 + es::SB1_FLAGS as u32 + f / 8;
            let v = self.m.bus.read8(a) | 1 << (f % 8);
            self.m.bus.write8(a, v);
        }
        self.m
            .bus
            .write16(sb1 + es::SB1_VARS as u32 + 2 * VAR_NATIONAL_DEX, 0x302);
        for (i, v) in [0u8, 1, 0xDA].into_iter().enumerate() {
            self.m.bus.write8(sb2 + SB2_DEX + i as u32, v);
        }
        // Seen counts only where the two copies in SaveBlock1 agree.
        for at in [
            sb2 + es::SB2_DEX_OWNED as u32,
            sb2 + es::SB2_DEX_SEEN as u32,
            sb1 + es::SB1_DEX_SEEN1 as u32,
            sb1 + es::SB1_DEX_SEEN2 as u32,
        ] {
            for i in 0..DEX_ENTRIES / 8 {
                self.m.bus.write8(at + i, 0xFF);
            }
            self.m
                .bus
                .write8(at + DEX_ENTRIES / 8, (1 << (DEX_ENTRIES % 8)) - 1);
        }
        self.m.bus.write8(START_MENU_CURSOR, cursor);
        Some((sb1, sb2))
    }

    /// Dex sweep step `c`: the Pokédex opened at entry `DEX_CHUNK * c`, its
    /// entry page, then Down through the chunk's entries, each page drawing
    /// the entry's footprint, picture and text.
    fn dex_sweep(&mut self, c: u32) {
        if self.menu_ready(0).is_none() {
            return;
        }
        let first = DEX_CHUNK * c;
        let last = DEX_ENTRIES.min(first + DEX_CHUNK);
        self.m.bus.write16(DEX_LAST_SELECTED, first as u16);
        self.m.bus.write32(DEX_LIST_ITEM, 0);
        self.sweeps += 1;
        let never = [u64::MAX];
        // Start and the Pokédex, then A until an entry page loads.
        let menu = es::Presses::parse("start@10,a@60").unwrap();
        self.play(200, 0, "dex", |f, _| menu.keys(f), &never);
        let item = |w: &Self| {
            let p = w.m.bus.read32(DEX_LIST_ITEM);
            (p != 0).then(|| w.m.bus.read16(p) as u32)
        };
        let press = |k: u16| move |f: u64, _: &mut Rng| if f < 2 { k } else { 0 };
        let mut budget = 300 * (1 + last - first) as u64 / DEX_STEP;
        while item(self).is_none() && budget > 0 {
            budget -= 1;
            self.play(DEX_STEP, 0, "dex", press(key::A), &never);
        }
        while item(self).is_some_and(|d| d < last) && budget > 0 {
            budget -= 1;
            self.play(DEX_STEP, 0, "dex", press(key::DOWN), &never);
        }
        // The last entry's page loads.
        self.play(150, 0, "dex", |_, _| 0, &[150]);
    }

    /// Bag sweep pass `p`: the pockets filled (the TMs and HMs, the
    /// berries, and the pass's share of the other items in the other
    /// three), the bag opened, and each pocket scrolled from top to bottom.
    fn bag_sweep(&mut self, p: u32) {
        let Some((sb1, sb2)) = self.menu_ready(2) else {
            return;
        };
        let k = self.m.bus.read32(sb2 + es::SB2_ENCRYPTION_KEY as u32) as u16;
        let general: Vec<u32> = (1..ITEMS)
            .filter(|i| !TMS.contains(i) && !BERRIES.contains(i))
            .collect();
        let per = es::SB1_BAG_POCKETS[0].1 + es::SB1_BAG_POCKETS[1].1 + es::SB1_BAG_POCKETS[2].1;
        let mut rest = general.iter().copied().skip(per * p as usize);
        for (pocket, &(at, slots)) in es::SB1_BAG_POCKETS.iter().enumerate() {
            let mut items: Box<dyn Iterator<Item = u32>> = match pocket {
                3 => Box::new(TMS),
                4 => Box::new(BERRIES),
                _ => Box::new(rest.by_ref().take(slots)),
            };
            for s in 0..slots {
                let item = items.next().unwrap_or(0) as u16;
                let qty = if item == 0 { 0 } else { 1 + s as u16 };
                let o = sb1 + (at + 4 * s) as u32;
                self.m.bus.write16(o, item);
                self.m.bus.write16(o + 2, qty ^ k);
            }
        }
        // The bag opens on the items pocket, each list at its top.
        self.m.bus.write8(BAG_POSITION + 5, 0);
        for i in 0..10 {
            self.m.bus.write16(BAG_POSITION + 8 + 2 * i, 0);
        }
        // The pockets in the bag's order: items, Poké Balls, TMs and HMs,
        // berries, key items; Down past each one's last slot, then Right.
        let mut script = String::from("start@10,a@60");
        let mut f = 300;
        for slots in [30, 16, 64, 46, 30] {
            let to = f + BAG_STEP * slots;
            let _ = write!(script, ",down@{f}-{to}/{BAG_STEP}+2,right@{}", to + 20);
            f = to + 80;
        }
        let presses = es::Presses::parse(&script).unwrap();
        self.sweeps += 1;
        self.play(f, 0, "bag", |f, _| presses.keys(f), &[f]);
    }

    /// PC sweep pass `p` (assets/fuzz-pc-box-sweep): the 14 boxes filled in
    /// RAM with the pass's share of every species and Unown's forms, the
    /// PC opened by its special in [`PC_PASSES`]' mode. A box pass puts
    /// each box on its own wallpaper, walks the cursor over each box's 30
    /// slots (each mon's picture and data drawn; in move items, the first
    /// one's item shown by Info) and on from the box's title to the next;
    /// then the box menu's Jump (the box popup) and Wallpaper, and the
    /// party. The Walda pass puts every box on Walda's wallpaper and
    /// scrolls from box to box, each with the next of its patterns and
    /// icons. A row of `scenes.tsv` a [`SCENE_CHUNK`].
    fn pc_sweep(&mut self, p: u32) {
        let Some((sb1, sb2)) = self.menu_ready(0) else {
            return;
        };
        let st = self.m.bus.read32(STORAGE_PTR);
        if !(0x0200_0000..0x0204_0000).contains(&st) {
            return;
        }
        let (chunk, option, walda) = PC_PASSES[p as usize];
        let ot = self.m.bus.read32(sb2 + 0x0A);
        let n = SPECIES - 1 + UNOWN_FORMS;
        self.m.bus.write8(st, 0);
        for i in 0..BOXES * BOX_SLOTS {
            let at = st + STORAGE_BOXES + BOX_MON as u32 * i;
            let j = BOXES * BOX_SLOTS * chunk + i;
            let mon = (j < n).then(|| {
                // Unown's letter is the personality's low two bits of each
                // byte, mod 28; A for the species' own slot.
                let (s, letter) = if j < SPECIES - 1 {
                    (1 + j as u16, 0)
                } else {
                    (UNOWN, 1 + j - (SPECIES - 1))
                };
                let personality = if s == UNOWN {
                    (letter & 3) | (letter >> 2 & 3) << 8 | (letter >> 4 & 3) << 16
                } else {
                    self.rng.next()
                };
                let level = 5 + self.rng.below(90) as u8;
                let mut mon = self.mon_of(s, level, personality, ot);
                if option == 3 {
                    es::set_held_item(&mut mon, (1 + j % (ITEMS - 1)) as u16);
                }
                mon
            });
            for k in 0..BOX_MON {
                let x = mon.map_or(0, |m| m[k]);
                self.m.bus.write8(at + k as u32, x);
            }
        }
        for b in 0..BOXES {
            let w = if walda {
                WALLPAPERS - 1
            } else {
                (BOXES * p + b) % WALLPAPERS
            };
            self.m.bus.write8(st + STORAGE_WALLPAPERS + b, w as u8);
        }
        self.m.bus.write8(sb1 + SB1_WALDA_PATTERN, 0);
        self.m.bus.write8(sb1 + SB1_WALDA_ICON, 0);
        let mut script = vec![SCR_SPECIAL];
        script.extend(SPECIAL_SHOW_PC.to_le_bytes());
        script.extend([SCR_WAITSTATE, SCR_END]);
        self.run_script(&script);
        // The presses: each a frame and a wait after it.
        let (mut s, mut f) = (String::new(), 60);
        let mut press = |k: &str, wait: u64| {
            let _ = write!(s, "{k}@{f}+2,");
            f += wait;
        };
        for _ in 0..option {
            press("down", 20);
        }
        // The fade and the storage's screen built.
        press("a", 240);
        if walda {
            press("start", PC_STEP);
        } else {
            if option == 3 {
                // The first slot's item: its menu, Info (move, bag, info,
                // cancel), the description slid in and out.
                for (k, wait) in [("a", 40), ("down", 20), ("down", 20), ("a", 120), ("a", 60)] {
                    press(k, wait);
                }
            }
            for b in 0..BOXES {
                // Right through each row (it wraps to the row's start), Down.
                for row in 0..BOX_SLOTS / BOX_COLUMNS {
                    for _ in 0..BOX_COLUMNS {
                        press("right", PC_STEP);
                    }
                    if row + 1 < BOX_SLOTS / BOX_COLUMNS {
                        press("down", PC_STEP);
                    }
                }
                // Start to the box's title, Right scrolls to the next box.
                press("start", PC_STEP);
                if b + 1 < BOXES {
                    press("right", 90);
                    press("down", PC_STEP);
                }
            }
            // On the title, A for the box menu: Jump (the box popup,
            // scrolled), cancelled; Wallpaper, its first theme, its first
            // wallpaper.
            for (k, wait) in [
                ("a", 40),
                ("a", 60),
                ("right", 40),
                ("right", 40),
                ("b", 60),
                ("a", 40),
                ("down", 20),
                ("a", 40),
                ("a", 40),
                ("a", 150),
            ] {
                press(k, wait);
            }
            // Down past the box's rows to the Party Pokémon button, A.
            for _ in 0..BOX_SLOTS / BOX_COLUMNS + 1 {
                press("down", PC_STEP);
            }
            press("a", 120);
            press("b", 120);
        }
        let name = format!("pc-{p}");
        self.sweeps += 1;
        let mut row = (0, 0);
        if !self.pc_play(&name, &s, f, &mut row) {
            return;
        }
        if walda {
            // Each scroll loads the next box's wallpaper: Walda's, with the
            // pattern and icon poked just before.
            let scroll = "right@0+2";
            for k in 0..WALDA_SCROLLS {
                self.m
                    .bus
                    .write8(sb1 + SB1_WALDA_PATTERN, (k % WALDA_PATTERNS) as u8);
                self.m
                    .bus
                    .write8(sb1 + SB1_WALDA_ICON, (k % WALDA_ICONS) as u8);
                if !self.pc_play(&name, scroll, 90, &mut row) {
                    return;
                }
            }
        }
        let (done, found) = row;
        if !done.is_multiple_of(SCENE_CHUNK) {
            let cb2 = self.m.bus.read32(CALLBACK2);
            let (g, m) = self.location();
            let _ = writeln!(self.scenes, "{name}\t{done}\t{cb2:#010x}\t{g}.{m}\t{found}");
        }
    }

    /// Play `presses` for `frames` frames of PC sweep `name`, `row` its
    /// (frames, found) so far: a row of `scenes.tsv` each time it reaches a
    /// [`SCENE_CHUNK`]. False on a fault.
    fn pc_play(&mut self, name: &str, presses: &str, frames: u64, row: &mut (u64, u64)) -> bool {
        let presses = es::Presses::parse(presses).unwrap();
        let mut at = 0;
        while at < frames {
            let done = row.0;
            let n = (SCENE_CHUNK - done % SCENE_CHUNK).min(frames - at);
            let from = at;
            let Some(k) = self.play(n, 0, name, |g, _| presses.keys(from + g), &[]) else {
                let _ = writeln!(self.scenes, "{name}\t{done}\tfault\t\t");
                return false;
            };
            at += n;
            row.0 += n;
            row.1 += k;
            if row.0.is_multiple_of(SCENE_CHUNK) {
                let cb2 = self.m.bus.read32(CALLBACK2);
                let (g, m) = self.location();
                let _ = writeln!(
                    self.scenes,
                    "{name}\t{}\t{cb2:#010x}\t{g}.{m}\t{}",
                    row.0, row.1
                );
                row.1 = 0;
            }
        }
        true
    }

    /// A sweep's save: map `i` of every group's, the bag's pockets full
    /// (consecutive items, starting where the last left off), the Pokédex
    /// complete, the system flags and badges on, six consecutive species.
    fn sweep_save(&mut self, i: u32) -> Vec<u8> {
        let mut b = self.base.clone();
        let (mut g, mut n) = (0, i);
        while g < MAP_COUNTS.len() && n >= MAP_COUNTS[g] as u32 {
            n -= MAP_COUNTS[g] as u32;
            g += 1;
        }
        let (g, n) = (g as u8, n as u8);
        let map = es::Map { group: g, num: n };
        es::put_warp(&mut b.sb1, es::SB1_LOCATION, map, (5, 5));
        es::put_warp(&mut b.sb1, es::SB1_CONTINUE_GAME_WARP, map, (5, 5));
        b.sb1[es::SB1_CONTINUE_GAME_WARP + 2] = 0;
        if let Some(l) = self.map_layout(g, n) {
            b.sb1[es::SB1_MAP_LAYOUT_ID..es::SB1_MAP_LAYOUT_ID + 2]
                .copy_from_slice(&l.to_le_bytes());
        }
        let k = u16::from_le_bytes(
            b.sb2[es::SB2_ENCRYPTION_KEY..es::SB2_ENCRYPTION_KEY + 2]
                .try_into()
                .unwrap(),
        );
        let mut item = 1 + (i * 37) % (ITEMS - 1);
        for (at, slots) in es::SB1_BAG_POCKETS {
            for s in 0..slots {
                let qty = (1 + self.rng.below(99) as u16) ^ k;
                let o = at + 4 * s;
                b.sb1[o..o + 2].copy_from_slice(&(item as u16).to_le_bytes());
                b.sb1[o + 2..o + 4].copy_from_slice(&qty.to_le_bytes());
                item = 1 + item % (ITEMS - 1);
            }
        }
        for off in [es::SB2_DEX_OWNED, es::SB2_DEX_SEEN] {
            b.sb2[off..off + 52].fill(0xFF);
        }
        for f in 0x860..0x8E0 {
            es::set_flag(&mut b.sb1, f);
        }
        let ot = u32::from_le_bytes(b.sb2[0x0A..0x0E].try_into().unwrap());
        b.sb1[es::SB1_PARTY_COUNT] = 6;
        for j in 0..6u32 {
            let s = 1 + (6 * i + j) % (SPECIES - 1);
            let level = 5 + self.rng.below(90) as u8;
            let p = self.rng.next();
            let mon = self.mon_of(s as u16, level, p, ot);
            let j = j as usize;
            b.sb1[es::SB1_PARTY + 100 * j..es::SB1_PARTY + 100 * (j + 1)].copy_from_slice(&mon);
        }
        es::flash(&b)
    }

    /// A scene's save: the furthest story point standing at `at` on `map`,
    /// six consecutive species from the first 149 (none the Frontier bans)
    /// at level 50, each with every master ribbon; the first knowing
    /// `first_move` first, if given.
    fn scene_save(
        &mut self,
        i: u32,
        map: es::Map,
        at: (i16, i16),
        first_move: Option<u16>,
    ) -> Vec<u8> {
        let mut b = self.story_base.clone();
        es::put_warp(&mut b.sb1, es::SB1_LOCATION, map, at);
        es::put_warp(&mut b.sb1, es::SB1_CONTINUE_GAME_WARP, map, at);
        b.sb1[es::SB1_CONTINUE_GAME_WARP + 2] = 0;
        if let Some(l) = self.map_layout(map.group, map.num) {
            b.sb1[es::SB1_MAP_LAYOUT_ID..es::SB1_MAP_LAYOUT_ID + 2]
                .copy_from_slice(&l.to_le_bytes());
        }
        let ot = u32::from_le_bytes(b.sb2[0x0A..0x0E].try_into().unwrap());
        b.sb1[es::SB1_PARTY_COUNT] = 6;
        for j in 0..6u32 {
            let s = 1 + (6 * i + j) % 149;
            let p = self.rng.next();
            let mut mon = match first_move {
                Some(mv) if j == 0 => {
                    let other = 1 + self.rng.below(MOVES - 1) as u16;
                    Self::mon_with(s as u16, 50, p, ot, [(mv, 20), (other, 20)])
                }
                _ => self.mon_of(s as u16, 50, p, ot),
            };
            es::add_ribbons(&mut mon, MASTER_RIBBONS);
            let j = j as usize;
            b.sb1[es::SB1_PARTY + 100 * j..es::SB1_PARTY + 100 * (j + 1)].copy_from_slice(&mon);
        }
        es::flash(&b)
    }

    /// Start the global script context on `script` (written to free EWRAM)
    /// as `ScriptContext_SetupScript` does.
    fn run_script(&mut self, script: &[u8]) {
        for (i, &x) in script.iter().enumerate() {
            self.m.bus.write8(SCRIPT_RAM + i as u32, x);
        }
        self.m.bus.write8(SCRIPT_CONTEXT, 0);
        self.m.bus.write8(SCRIPT_CONTEXT + 1, 1);
        self.m.bus.write32(SCRIPT_CONTEXT + 8, SCRIPT_RAM);
        self.m.bus.write8(SCRIPT_STATUS, 0);
        self.m.bus.write8(LOCK_FIELD_CONTROLS, 1);
    }

    /// Scene sweep step `i` (assets/fuzz-contest-frontier-sweeps): the
    /// contests, each category at each rank, entered as the lobby's
    /// receptionist enters one once the choices are made; then each
    /// Frontier attendant's script. A press every 8 frames takes each
    /// menu's first choice, but on the screens of [`CB2_UPDATE_PARTY_MENU`]
    /// and the rest, which have their own. Writes a row of `scenes.tsv` a
    /// [`SCENE_CHUNK`].
    fn scene(&mut self, reset: &[u8], i: u32) {
        let contests = CONTEST_CATEGORIES * CONTEST_RANKS;
        let (name, frames, lobby) = if i < contests {
            let (category, rank) = (
                (i % CONTEST_CATEGORIES) as u16,
                (i / CONTEST_CATEGORIES) as u16,
            );
            let mv = CONTEST_MOVES[(i % CONTEST_MOVES.len() as u32) as usize];
            let s = self.scene_save(i, CONTEST_LOBBY, CONTEST_AT, Some(mv));
            self.boot(reset, &s, "scene");
            let mut script = Vec::new();
            let special = |v: &mut Vec<u8>, n: u16| {
                v.push(SCR_SPECIAL);
                v.extend(n.to_le_bytes());
            };
            let setvar = |v: &mut Vec<u8>, var: u16, x: u16| {
                v.push(SCR_SETVAR);
                v.extend(var.to_le_bytes());
                v.extend(x.to_le_bytes());
            };
            let call = |v: &mut Vec<u8>, at: u32| {
                v.push(SCR_CALL);
                v.extend(at.to_le_bytes());
            };
            special(&mut script, SPECIAL_CLEAR_LINK_CONTEST_FLAGS);
            setvar(&mut script, VAR_CONTEST_RANK, rank);
            setvar(&mut script, VAR_CONTEST_CATEGORY, category);
            special(&mut script, SPECIAL_TRY_ENTER_CONTEST_MON);
            special(&mut script, SPECIAL_SET_CONTEST_TRAINER_GFX_IDS);
            call(&mut script, SCR_SET_PLAYER_GFX);
            call(&mut script, SCR_SET_CONTEST_TYPE);
            setvar(&mut script, VAR_CONTEST_HALL_STATE, 1);
            call(&mut script, SCR_WARP_TO_CONTEST_HALL);
            script.extend([SCR_WAITSTATE, SCR_END]);
            self.m.bus.write8(CONTEST_MON_INDEX, 0);
            self.run_script(&script);
            (format!("contest-{category}-{rank}"), CONTEST_FRAMES, None)
        } else {
            let (name, num, (x, y), script) = FRONTIER[(i - contests) as usize];
            let map = es::Map {
                group: FRONTIER_GROUP,
                num,
            };
            // Across the counter from the attendant, its script started.
            let s = self.scene_save(i, map, (x, y + 2), None);
            self.boot(reset, &s, "scene");
            let mut goto = vec![SCR_GOTO];
            goto.extend(script.to_le_bytes());
            self.run_script(&goto);
            (
                name.to_string(),
                FRONTIER_FRAMES,
                Some((FRONTIER_GROUP, num)),
            )
        };
        self.sweeps += 1;
        let (mut done, mut found) = (0, 0);
        while done < frames {
            // A [`WINDOW`] at a time, the presses by the screen it starts on:
            // a press every 8 frames from `pattern`, or a random one held 16
            // frames (A one time in two, else a direction, to walk).
            let cb2 = self.m.bus.read32(CALLBACK2);
            let mut random = false;
            let pattern: &[u16] = match cb2 {
                CB2_UPDATE_PARTY_MENU if self.m.bus.read8(PARTY_MENU_TYPE) & 0xF == 1 => {
                    &[key::DOWN, key::A, key::A]
                }
                CB2_UPDATE_PARTY_MENU => {
                    self.choose_half();
                    &[key::A]
                }
                CB2_SUMMARY_SCREEN | CB2_INIT_SUMMARY_SCREEN => &[key::B, key::LEFT, key::A],
                CB2_TOURNEY_TREE => &[key::A, key::B, key::DOWN, key::B],
                CB2_FACTORY_SELECT_SCREEN | CB2_FACTORY_SWAP_SCREEN => {
                    random = true;
                    &[]
                }
                CB2_OVERWORLD if lobby.is_some_and(|l| self.location() != l) => {
                    random = true;
                    &[]
                }
                _ => &[key::A],
            };
            let (at, mut held) = (done, 0);
            let keys = |f: u64, r: &mut Rng| {
                let t = at + f;
                if random {
                    if t.is_multiple_of(16) {
                        held = [
                            key::A,
                            key::A,
                            key::A,
                            key::A,
                            key::UP,
                            key::DOWN,
                            key::LEFT,
                            key::RIGHT,
                        ][r.below(8) as usize];
                    }
                    if held == key::A && t % 16 >= 2 {
                        0
                    } else {
                        held
                    }
                } else if t % 8 < 2 {
                    pattern[(t / 8) as usize % pattern.len()]
                } else {
                    0
                }
            };
            let Some(n) = self.play(WINDOW, 0, &name, keys, &[]) else {
                let _ = writeln!(self.scenes, "{name}\t{done}\tfault\t\t");
                return;
            };
            found += n;
            done += WINDOW;
            if done % SCENE_CHUNK == 0 || done >= frames {
                let (g, m) = self.location();
                let _ = writeln!(self.scenes, "{name}\t{done}\t{cb2:#010x}\t{g}.{m}\t{found}");
                found = 0;
            }
        }
    }

    /// In a Frontier challenge's choose-half party menu, before any mon is
    /// entered: enter slots 2 to `gSpecialVar_0x8005` (the number to
    /// choose) as the game would, so the next A, on the first slot,
    /// enters it and moves the cursor to Confirm.
    fn choose_half(&mut self) {
        let max = self.m.bus.read16(SPECIAL_VAR_8005).min(4) as u32;
        if max < 2 || self.m.bus.read8(SELECTED_ORDER) != 0 {
            return;
        }
        for j in 0..max - 1 {
            self.m.bus.write8(SELECTED_ORDER + j, 2 + j as u8);
        }
    }

    /// Boot `save` from reset and continue into the overworld.
    fn boot(&mut self, reset: &[u8], save: &[u8], why: &str) {
        self.m.load_state(reset).expect("our own state loads");
        self.m.bus.save.data[..save.len()].copy_from_slice(save);
        let presses = es::Presses::parse(CONTINUE).unwrap();
        self.boots += 1;
        self.play(
            CONTINUE_END,
            0,
            why,
            |f, _| presses.keys(f),
            &[CONTINUE_END],
        );
    }

    /// Run on from a TAS's mGBA state, just loaded, and keep it.
    fn boot_state(&mut self) {
        self.boots += 1;
        self.play(STATE_SETTLE, 0, "story", |_, _| 0, &[STATE_SETTLE]);
    }

    fn mon(&mut self, ot: u32) -> [u8; 100] {
        let s = 1 + self.rng.below(SPECIES - 1) as u16;
        let level = 1 + self.rng.below(100) as u8;
        let p = self.rng.next();
        self.mon_of(s, level, p, ot)
    }

    /// Species `id` at `level` with `personality`, two random moves.
    fn mon_of(&mut self, id: u16, level: u8, personality: u32, ot: u32) -> [u8; 100] {
        let r = &mut self.rng;
        let moves = [
            (1 + r.below(MOVES - 1) as u16, 20),
            (1 + r.below(MOVES - 1) as u16, 20),
        ];
        Self::mon_with(id, level, personality, ot, moves)
    }

    /// Species `id` at `level` with `personality` and `moves` (id and PP).
    fn mon_with(id: u16, level: u8, personality: u32, ot: u32, moves: [(u16, u8); 2]) -> [u8; 100] {
        let species = es::Species {
            id,
            dex: 1,
            name: "MON",
            base: [50; 6],
            growth: es::Growth::MediumFast,
            moves,
        };
        let m = es::Mon {
            species,
            level,
            personality,
        };
        let t = es::Trainer {
            file: "",
            name: "FUZZ",
            female: false,
            id: ot,
            party: Vec::new(),
        };
        es::encode_mon(&m, &t)
    }

    fn map_layout(&self, g: u8, n: u8) -> Option<u16> {
        let rd = |a: u32| -> Option<u32> {
            let o = a.checked_sub(ROM)? as usize;
            Some(u32::from_le_bytes(self.rom.get(o..o + 4)?.try_into().ok()?))
        };
        let group = rd(MAP_GROUPS + 4 * g as u32)?;
        let header = rd(group + 4 * n as u32)?;
        Some(rd(header + 0x12)? as u16)
    }

    /// The crafted save, mutated: party, map, flags, bag, Pokédex, vars.
    fn mutated_save(&mut self) -> Vec<u8> {
        let mut b = self.base.clone();
        let ot = u32::from_le_bytes(b.sb2[0x0A..0x0E].try_into().unwrap());
        if self.rng.chance(70) {
            let n = 1 + self.rng.below(6) as usize;
            b.sb1[es::SB1_PARTY_COUNT] = n as u8;
            for i in 0..n {
                let mon = self.mon(ot);
                b.sb1[es::SB1_PARTY + 100 * i..es::SB1_PARTY + 100 * (i + 1)].copy_from_slice(&mon);
            }
        }
        if self.rng.chance(60) {
            let g = self.rng.below(MAP_COUNTS.len() as u32) as u8;
            let n = self.rng.below(MAP_COUNTS[g as usize] as u32) as u8;
            let map = es::Map { group: g, num: n };
            let pos = (2 + self.rng.below(12) as i16, 2 + self.rng.below(12) as i16);
            es::put_warp(&mut b.sb1, es::SB1_LOCATION, map, pos);
            es::put_warp(&mut b.sb1, es::SB1_CONTINUE_GAME_WARP, map, pos);
            // Arrive at the map's first warp, a place the player can stand.
            b.sb1[es::SB1_CONTINUE_GAME_WARP + 2] = 0;
            if let Some(l) = self.map_layout(g, n) {
                b.sb1[es::SB1_MAP_LAYOUT_ID..es::SB1_MAP_LAYOUT_ID + 2]
                    .copy_from_slice(&l.to_le_bytes());
            }
        }
        if self.rng.chance(60) {
            for _ in 0..1 + self.rng.below(200) {
                let f = self.rng.below(300 * 8) as u16;
                es::set_flag(&mut b.sb1, f);
            }
            if self.rng.chance(50) {
                // The system flags and badges (FLAG_SYS_POKEMON_GET on).
                for f in 0x860..0x8E0 {
                    if self.rng.chance(50) {
                        es::set_flag(&mut b.sb1, f);
                    }
                }
            }
        }
        if self.rng.chance(60) {
            let k = u16::from_le_bytes(
                b.sb2[es::SB2_ENCRYPTION_KEY..es::SB2_ENCRYPTION_KEY + 2]
                    .try_into()
                    .unwrap(),
            );
            for (at, slots) in es::SB1_BAG_POCKETS {
                for s in 0..slots.min(self.rng.below(12) as usize) {
                    let item = 1 + self.rng.below(ITEMS - 1) as u16;
                    let qty = (1 + self.rng.below(99) as u16) ^ k;
                    let o = at + 4 * s;
                    b.sb1[o..o + 2].copy_from_slice(&item.to_le_bytes());
                    b.sb1[o + 2..o + 4].copy_from_slice(&qty.to_le_bytes());
                }
            }
        }
        if self.rng.chance(50) {
            for off in [es::SB2_DEX_OWNED, es::SB2_DEX_SEEN] {
                for i in 0..52 {
                    b.sb2[off + i] |= self.rng.next() as u8;
                }
            }
        }
        if self.rng.chance(30) {
            for _ in 0..1 + self.rng.below(20) {
                let v = es::SB1_VARS + 2 * self.rng.below(256) as usize;
                let x = self.rng.below(16) as u16;
                b.sb1[v..v + 2].copy_from_slice(&x.to_le_bytes());
            }
        }
        es::flash(&b)
    }

    /// The game in RAM, poked: the party, a few flags, or the bag.
    fn poke(&mut self) {
        let sb1 = self.m.bus.read32(es::SB1_PTR);
        let sb2 = self.m.bus.read32(SB2_PTR);
        if !(0x0200_0000..0x0204_0000).contains(&sb1) || !(0x0200_0000..0x0204_0000).contains(&sb2)
        {
            return;
        }
        self.pokes += 1;
        match self.rng.below(3) {
            0 => {
                let ot = self.m.bus.read32(sb2 + 0x0A);
                let n = 1 + self.rng.below(6);
                self.m.bus.write8(PARTY_COUNT, n as u8);
                for i in 0..n {
                    let mon = self.mon(ot);
                    for (j, &x) in mon.iter().enumerate() {
                        self.m.bus.write8(PARTY + 100 * i + j as u32, x);
                    }
                }
            }
            1 => {
                for _ in 0..1 + self.rng.below(20) {
                    let f = self.rng.below(300 * 8);
                    let a = sb1 + es::SB1_FLAGS as u32 + f / 8;
                    let v = self.m.bus.read8(a) ^ 1 << (f % 8);
                    self.m.bus.write8(a, v);
                }
            }
            _ => {
                let k = self.m.bus.read32(sb2 + es::SB2_ENCRYPTION_KEY as u32) as u16;
                for (at, slots) in es::SB1_BAG_POCKETS {
                    for s in 0..slots.min(self.rng.below(8) as usize) {
                        let item = 1 + self.rng.below(ITEMS - 1) as u16;
                        let qty = (1 + self.rng.below(99) as u16) ^ k;
                        let o = sb1 + (at + 4 * s) as u32;
                        self.m.bus.write16(o, item);
                        self.m.bus.write16(o + 2, qty);
                    }
                }
            }
        }
    }
}

/// `gbaa-fuzz worker`: run for `secs` seconds, writing into `out`,
/// seeded with the story-point saves in `story`, if given.
#[expect(clippy::too_many_arguments)]
pub fn run(
    rom_path: &Path,
    save_path: &Path,
    story: Option<&Path>,
    out: &Path,
    secs: u64,
    seed: u64,
    index: u32,
    of: u32,
    pc_only: bool,
) -> Result<(), String> {
    let rom = std::fs::read(rom_path).map_err(|e| format!("{}: {e}", rom_path.display()))?;
    if rom.get(0xAC..0xB0) != Some(b"BPEE") {
        return Err("not Pokémon Emerald (US): its game code isn't BPEE".into());
    }
    let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
    let save = std::fs::read(save_path).map_err(|e| format!("{}: {e}", save_path.display()))?;
    let mut seeds = Vec::new();
    if let Some(dir) = story {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "sav" || x == "ss"))
            .collect();
        paths.sort();
        for p in paths {
            let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let state = p.extension().is_some_and(|x| x == "ss");
            seeds.push((p, state, data));
        }
    }
    let base = es::read(&save).ok_or("the save has no whole slot")?;
    let story_base = match seeds.iter().rev().find(|(_, state, _)| !state) {
        Some((_, _, s)) => es::read(s).ok_or("the last story save has no whole slot")?,
        None => base.clone(),
    };
    let c = Core::new(rom, out, seed, index, of, screen, place)?;
    let mut scenes = std::fs::File::create(out.join("scenes.tsv"))
        .map(std::io::BufWriter::new)
        .map_err(|e| format!("scenes.tsv: {e}"))?;
    let _ = writeln!(scenes, "scene\tframe\tcallback2\tmap\tfound");
    let mut w = Worker {
        c,
        rom,
        base,
        story_base,
        scenes,
        overworld: Vec::new(),
        swept: [0; 3],
    };
    let reset = w.snapshot();
    // The new-game route (units/emerald-newgame.toml): what a route finds.
    let route = es::Presses::parse("start@300,a@900,a@1100-5380/20").unwrap();
    w.play(ROUTE_END, 0, "route", |f, _| route.keys(f + 1), &ROUTE_KEEP);
    w.cov.phase = "seed";
    w.boot(&reset, &save, "save");
    w.overworld = w.snapshot();
    // This worker's share of the Pokédex's entries and the bag passes.
    w.cov.phase = "dex";
    let mut c = w.index;
    while c < DEX_ENTRIES.div_ceil(DEX_CHUNK) {
        w.dex_sweep(c);
        c += w.of;
    }
    w.cov.phase = "bag";
    let mut p = w.index;
    while p < BAG_PASSES {
        w.bag_sweep(p);
        p += w.of;
    }
    // This worker's share of the PC passes.
    w.cov.phase = "pc";
    let mut p = w.index;
    while p < PC_PASSES.len() as u32 {
        w.pc_sweep(p);
        p += w.of;
    }
    if pc_only {
        let _ = w.scenes.flush();
        w.stat();
        return Ok(());
    }
    // This worker's share of the moves, each used in a battle.
    w.cov.phase = "moves";
    let mut mv = 1 + w.index;
    while mv < MOVES {
        w.move_battle(mv, false);
        mv += w.of;
    }
    // And each used by the foe, on the lead (assets/fuzz-move-sweep-foe-side).
    w.cov.phase = "foe-moves";
    let mut mv = 1 + w.index;
    while mv < MOVES {
        w.move_battle(mv, true);
        mv += w.of;
    }
    // The story seeds, each worker booting them all.
    w.cov.phase = "story";
    for (p, state, s) in &seeds {
        if !state {
            w.boot(&reset, s, "story");
        } else if let Err(e) = w.m.load_mgba_state(s) {
            eprintln!("{}: {e}", p.display());
        } else {
            w.boot_state();
        }
    }
    // This worker's share of the contests and the Frontier's challenges.
    w.cov.phase = "scene";
    let mut i = w.index;
    while i < CONTEST_CATEGORIES * CONTEST_RANKS + FRONTIER.len() as u32 {
        w.scene(&reset, i);
        i += w.of;
    }
    let _ = w.scenes.flush();
    w.cov.phase = "fuzz";
    while w.cov.start.elapsed().as_secs() < secs {
        w.tick();
        // Sweeps: a battle in three iterations, a sweep save in ten,
        // while this worker's share of each list lasts (assets/fuzz-to-90:
        // at one in five and one in twenty, 15 workers were still 15%
        // short of the lists after an hour).
        let roll = w.rng.below(100);
        if roll < 33 {
            let wild = w.rng.chance(60);
            if wild && let Some(i) = w.next_item(0, 2 * (SPECIES - 1)) {
                w.battle(Some(i), None);
                continue;
            }
            if let Some(t) = w.next_item(1, TRAINERS - 1) {
                w.battle(None, Some(1 + t));
                continue;
            }
        } else if roll < 43 {
            let maps = MAP_COUNTS.iter().map(|&n| n as u32).sum();
            if let Some(i) = w.next_item(2, maps) {
                let s = w.sweep_save(i);
                w.sweeps += 1;
                w.boot(&reset, &s, "sweep");
                continue;
            }
        }
        if w.rng.chance(10) || w.corpus.is_empty() {
            let s = w.mutated_save();
            w.boot(&reset, &s, "boot");
            continue;
        }
        let id = w.load_pick();
        let why = if w.rng.chance(15) {
            w.poke();
            "poke"
        } else {
            "play"
        };
        w.random_run(id, why);
    }
    w.stat();
    Ok(())
}
