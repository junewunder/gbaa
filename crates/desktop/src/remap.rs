//! Remapping the pads and the keyboard, for every game and for one: the
//! mapping file ([`gamepad::config_path`], or `--pad-config`) has
//! `button = controls` lines (the pads') and `key button = keys` lines
//! (the keyboard's) for every game, then sections for single ROMs, each
//! its lines until the next:
//!
//! ```text
//! a = south
//! key a = j
//! [title POKEMON EMER]
//! fast = rb
//! [hash 0123456789abcdef]  # this one hack
//! a = east
//! key a = space
//! ```
//!
//! A ROM is read through the default, then the lines for every game, then
//! its title's section (the header's 12 characters), then its hash's (the
//! whole ROM's FNV-1a 64, as recompiled builds check it), each replacing
//! the buttons it names: a pads' line the pads' controls, a keyboard's
//! line its keys. The menu's remap screen ([`crate::menu`]) edits
//! the file in place, line by line, so its comments and order stay.

use crate::gamepad::{self, BUTTONS, Binding, Control, Mapping};
use runtime::io::key;
use sdl3::gamepad::{Axis, Button};
use sdl3::keyboard::Scancode;
use std::path::{Path, PathBuf};

/// What a ROM's sections are found by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RomId {
    /// The whole ROM's hash ([`runtime::dispatch::rom_hash`]).
    pub hash: u64,
    /// The header's title, its padding and any spaces round it trimmed.
    pub title: String,
}

impl RomId {
    pub fn of(rom: &[u8]) -> RomId {
        let title = rom
            .get(0xA0..0xAC)
            .unwrap_or(&[])
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| {
                if b == b' ' || b.is_ascii_graphic() {
                    b as char
                } else {
                    '?'
                }
            })
            .collect::<String>()
            .trim()
            .to_owned();
        RomId {
            hash: runtime::dispatch::rom_hash(rom),
            title,
        }
    }
}

/// A part of the mapping file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    /// The lines before any section: every game.
    All,
    /// `[title …]`: ROMs with this title.
    Title(String),
    /// `[hash …]`: the ROM with this hash, in 16 hex digits.
    Hash(u64),
}

impl Section {
    /// A ROM's sections, the later winning.
    pub fn of(rom: &RomId) -> [Section; 3] {
        [
            Section::All,
            Section::Title(rom.title.clone()),
            Section::Hash(rom.hash),
        ]
    }

    fn header(&self) -> String {
        match self {
            Section::All => String::new(),
            Section::Title(t) => format!("[title {t}]"),
            Section::Hash(h) => format!("[hash {h:016x}]"),
        }
    }

    /// The section a `[…]` line (comment stripped) starts.
    fn parse(line: &str) -> Result<Section, String> {
        let inner = line
            .strip_prefix('[')
            .and_then(|l| l.strip_suffix(']'))
            .ok_or("expected `[title …]` or `[hash …]`")?;
        let (kind, key) = inner.split_once(' ').unwrap_or((inner, ""));
        let key = key.trim();
        match kind {
            "title" if !key.is_empty() => Ok(Section::Title(key.to_owned())),
            "hash" if key.len() == 16 => u64::from_str_radix(key, 16)
                .map(Section::Hash)
                .map_err(|_| format!("bad hash {key:?}")),
            "hash" => Err(format!("bad hash {key:?} (16 hex digits)")),
            _ => Err("expected `[title …]` or `[hash …]`".into()),
        }
    }
}

/// A line with its comment cut off, trimmed.
fn bare(line: &str) -> &str {
    line.split('#').next().unwrap_or("").trim()
}

/// The mapping file, as its lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    lines: Vec<String>,
}

impl Config {
    /// Parse a mapping file, checking every section.
    pub fn parse(text: &str) -> Result<Config, String> {
        let c = Config {
            lines: text.lines().map(str::to_owned).collect(),
        };
        let sections = c.sections()?;
        for (i, (s, _)) in sections.iter().enumerate() {
            if sections[..i].iter().any(|(t, _)| t == s) {
                return Err(format!("{} named twice", s.header()));
            }
        }
        for (_, body) in &sections {
            Mapping::lines(body.clone().map(|n| (n, c.lines[n].as_str())))?;
        }
        Ok(c)
    }

    /// The file's text.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// Each section, with the lines of its body (after its header).
    fn sections(&self) -> Result<Vec<(Section, std::ops::Range<usize>)>, String> {
        let mut out = vec![(Section::All, 0..self.lines.len())];
        for (n, line) in self.lines.iter().enumerate() {
            let line = bare(line);
            if line.starts_with('[') {
                let s = Section::parse(line).map_err(|e| format!("line {}: {e}", n + 1))?;
                if let Some((_, body)) = out.last_mut() {
                    body.end = n;
                }
                out.push((s, n + 1..self.lines.len()));
            }
        }
        Ok(out)
    }

    fn body(&self, s: &Section) -> Option<std::ops::Range<usize>> {
        let sections = self.sections().expect("a parsed file's sections parse");
        sections.into_iter().find(|(t, _)| t == s).map(|(_, b)| b)
    }

    /// The button line `n` binds, and whether it's the keyboard's line,
    /// if it's one.
    fn button(&self, n: usize) -> Option<(u16, bool)> {
        Mapping::lines(std::iter::once((n, self.lines[n].as_str())))
            .ok()?
            .first()
            .map(|l| (l.bit, l.keys))
    }

    /// The lines in section `s`.
    pub fn bindings(&self, s: &Section) -> Vec<Binding> {
        self.body(s).map_or(Vec::new(), |b| {
            Mapping::lines(b.map(|n| (n, self.lines[n].as_str())))
                .expect("a parsed file's lines parse")
        })
    }

    /// Whether section `s` names `bit`, on a keyboard line (`keys`) or a
    /// pads' one.
    pub fn names(&self, s: &Section, bit: u16, keys: bool) -> bool {
        self.bindings(s)
            .iter()
            .any(|l| l.bit == bit && l.keys == keys)
    }

    /// The mapping for `rom`: the default, then each of its sections.
    pub fn mapping(&self, rom: &RomId) -> Mapping {
        Section::of(rom)
            .iter()
            .fold(Mapping::default(), |m, s| m.with_lines(self.bindings(s)))
    }

    /// Section `s` at the end, its header followed by `# comment` unless
    /// that's empty. Returns its (empty) body.
    fn add(&mut self, s: &Section, comment: &str) -> std::ops::Range<usize> {
        if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
            self.lines.push(String::new());
        }
        self.lines.push(match comment {
            "" => s.header(),
            c => format!("{}  # {c}", s.header()),
        });
        self.lines.len()..self.lines.len()
    }

    /// Bind `bit` in section `s` to `controls`, keys (on its keyboard
    /// line) or not (a section that hasn't one is added at the end), or
    /// with `None` drop that line there, so what's before it binds it
    /// again.
    pub fn set(&mut self, s: &Section, bit: u16, keys: bool, controls: Option<&[Control]>) {
        let body = match (self.body(s), controls) {
            (Some(b), _) => b,
            (None, None) => return,
            (None, Some(_)) => self.add(s, ""),
        };
        let at = body.clone().find(|&n| self.button(n) == Some((bit, keys)));
        let Some(controls) = controls else {
            if let Some(n) = at {
                self.lines.remove(n);
            }
            return;
        };
        let name = BUTTONS
            .iter()
            .find(|(_, b)| *b == bit)
            .map_or("?", |(n, _)| n);
        let names: Vec<_> = controls.iter().map(|&c| gamepad::control_name(c)).collect();
        let key = if keys { "key " } else { "" };
        let line = format!("{key}{name} = {}", names.join(" "))
            .trim_end()
            .to_owned();
        match at {
            Some(n) => self.lines[n] = line,
            None => {
                // After the body's last line that isn't blank.
                let n = body
                    .clone()
                    .rev()
                    .find(|&n| !self.lines[n].trim().is_empty())
                    .map_or(body.start, |n| n + 1);
                self.lines.insert(n, line);
            }
        }
    }
}

/// The GBA's directions, in a [`Four`]'s order.
pub const DIRECTIONS: [u16; 4] = [key::UP, key::DOWN, key::LEFT, key::RIGHT];

/// Four controls that make a whole pad of directions: a stick, the D-pad,
/// or four keys. The remap screen shows the directions as one row of
/// these when it can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Four {
    pub name: &'static str,
    /// Up, down, left and right's.
    pub controls: [Control; 4],
}

const fn four_buttons(name: &'static str, b: [Button; 4]) -> Four {
    let c = Control::Button;
    Four {
        name,
        controls: [c(b[0]), c(b[1]), c(b[2]), c(b[3])],
    }
}

const fn four_stick(name: &'static str, x: Axis, y: Axis) -> Four {
    Four {
        name,
        controls: [
            Control::Stick(y, false),
            Control::Stick(y, true),
            Control::Stick(x, false),
            Control::Stick(x, true),
        ],
    }
}

const fn four_keys(name: &'static str, k: [Scancode; 4]) -> Four {
    let c = Control::Key;
    Four {
        name,
        controls: [c(k[0]), c(k[1]), c(k[2]), c(k[3])],
    }
}

/// Every [`Four`] the remap screen knows.
pub const FOURS: [Four; 7] = [
    four_buttons(
        "D-pad",
        [
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ],
    ),
    four_stick("left stick", Axis::LeftX, Axis::LeftY),
    four_stick("right stick", Axis::RightX, Axis::RightY),
    four_keys(
        "arrow keys",
        [
            Scancode::Up,
            Scancode::Down,
            Scancode::Left,
            Scancode::Right,
        ],
    ),
    four_keys("WASD", [Scancode::W, Scancode::S, Scancode::A, Scancode::D]),
    four_keys("IJKL", [Scancode::I, Scancode::K, Scancode::J, Scancode::L]),
    four_keys(
        "keypad 8 2 4 6",
        [Scancode::Kp8, Scancode::Kp2, Scancode::Kp4, Scancode::Kp6],
    ),
];

impl Four {
    /// The [`Four`] `c` is one of.
    pub fn of(c: Control) -> Option<&'static Four> {
        FOURS.iter().find(|f| f.controls.contains(&c))
    }

    /// The directions in `m` (keys or pads') as whole [`Four`]s, in the
    /// order up's are, if they're only that: none if a direction has a
    /// control no [`Four`] of the others shares.
    pub fn split(m: &Mapping, keys: bool) -> Option<Vec<&'static Four>> {
        let dirs = DIRECTIONS.map(|bit| m.of(bit, keys));
        let mut out: Vec<&'static Four> = Vec::new();
        for &c in &dirs[0] {
            let f = Four::of(c)?;
            if f.controls[0] != c || out.contains(&f) {
                return None;
            }
            out.push(f);
        }
        let same = |i: usize| {
            let mut want: Vec<_> = out.iter().map(|f| f.controls[i]).collect();
            let mut have = dirs[i].clone();
            let key = |c: &Control| format!("{c:?}");
            want.sort_by_key(key);
            have.sort_by_key(key);
            want == have
        };
        (1..4).all(same).then_some(out)
    }
}

/// The pads' mapping as the remap screen edits it: the file, where it's
/// saved, and the ROM it's for.
#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub config: Config,
    pub rom: RomId,
    /// Where changes are saved: none, in tests.
    pub path: Option<PathBuf>,
}

impl Controls {
    /// The mapping file at `path`, for `rom`. A missing file is an empty
    /// one unless `required` (it was named on the command line).
    pub fn load(path: &Path, required: bool, rom: RomId) -> Result<Controls, String> {
        let config = match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Config::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Controls {
            config,
            rom,
            path: Some(path.to_owned()),
        })
    }

    pub fn mapping(&self) -> Mapping {
        self.config.mapping(&self.rom)
    }

    /// Where the remap screen's changes go: this ROM's hash's section, or
    /// every game's.
    pub fn section(&self, this_game: bool) -> Section {
        if this_game {
            Section::Hash(self.rom.hash)
        } else {
            Section::All
        }
    }

    /// The mapping the remap screen shows: this ROM's, or every game's
    /// (the default and the lines for every game, not this ROM's
    /// sections).
    pub fn view(&self, this_game: bool) -> Mapping {
        if this_game {
            self.mapping()
        } else {
            Mapping::default().with_lines(self.config.bindings(&Section::All))
        }
    }

    /// Whether this ROM's own section binds `bit` (its keys or its pads'
    /// controls): what "this game" changed.
    pub fn changed_here(&self, bit: u16, keys: bool) -> bool {
        self.config.names(&Section::Hash(self.rom.hash), bit, keys)
    }

    /// Bind `bit` to `controls` where `this_game` says, its section added
    /// (named for the game, for whoever reads the file) if it's new.
    fn put(&mut self, this_game: bool, bit: u16, keys: bool, controls: &[Control]) {
        let s = self.section(this_game);
        if s != Section::All && self.config.body(&s).is_none() {
            self.config.add(&s, &self.rom.title);
        }
        self.config.set(&s, bit, keys, Some(controls));
    }

    /// Add `c` to what presses `bit`, for this game or every game, taking
    /// it from any other button it presses there. Returns those buttons.
    pub fn add(&mut self, this_game: bool, bit: u16, c: Control) -> Vec<u16> {
        let m = self.view(this_game);
        let keys = c.is_key();
        let mut had = m.of(bit, keys);
        if had.contains(&c) {
            return Vec::new();
        }
        let mut taken = Vec::new();
        for &(_, b) in &BUTTONS {
            let on = m.of(b, keys);
            if b != bit && on.contains(&c) {
                let rest: Vec<_> = on.into_iter().filter(|&h| h != c).collect();
                self.put(this_game, b, keys, &rest);
                taken.push(b);
            }
        }
        had.push(c);
        self.put(this_game, bit, keys, &had);
        taken
    }

    /// Take `c` off `bit`, for this game or every game.
    pub fn remove(&mut self, this_game: bool, bit: u16, c: Control) {
        let had = self.view(this_game).of(bit, c.is_key());
        if had.contains(&c) {
            let rest: Vec<_> = had.into_iter().filter(|&h| h != c).collect();
            self.put(this_game, bit, c.is_key(), &rest);
        }
    }

    /// Add a whole [`Four`] to the four directions, each its own way.
    /// Returns the buttons its controls were taken from.
    pub fn add_four(&mut self, this_game: bool, four: &Four) -> Vec<u16> {
        let mut taken = Vec::new();
        for (&bit, &c) in DIRECTIONS.iter().zip(&four.controls) {
            for b in self.add(this_game, bit, c) {
                if !taken.contains(&b) {
                    taken.push(b);
                }
            }
        }
        taken
    }

    /// Take a whole [`Four`] off the four directions.
    pub fn remove_four(&mut self, this_game: bool, four: &Four) {
        for (&bit, &c) in DIRECTIONS.iter().zip(&four.controls) {
            self.remove(this_game, bit, c);
        }
    }

    /// Drop this ROM's own line for `bit`, so every game's binds it again.
    pub fn use_every_game(&mut self, bit: u16, keys: bool) {
        self.config
            .set(&Section::Hash(self.rom.hash), bit, keys, None);
    }

    /// Drop every line of one kind (keys or pads') where `this_game`
    /// says: this game back to every game's, or every game back to the
    /// default.
    pub fn reset(&mut self, this_game: bool, keys: bool) {
        let s = self.section(this_game);
        for &(_, bit) in &BUTTONS {
            self.config.set(&s, bit, keys, None);
        }
    }

    /// Write the file, if it has a place. Returns where it went.
    pub fn save(&self) -> Result<Option<PathBuf>, String> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::fs::write(path, self.config.text()).map_err(err)?;
        Ok(Some(path.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMERALD: RomId = RomId {
        hash: 0x0123_4567_89ab_cdef,
        title: String::new(),
    };

    fn emerald() -> RomId {
        rom(EMERALD.hash, "POKEMON EMER")
    }

    fn rom(hash: u64, title: &str) -> RomId {
        RomId {
            hash,
            title: title.into(),
        }
    }

    const SOUTH: Control = Control::Button(Button::South);
    const EAST: Control = Control::Button(Button::East);
    const RB: Control = Control::Button(Button::RightShoulder);

    #[test]
    fn a_roms_override_wins_for_it_only() {
        let c = Config::parse(
            "# mine\na = south\nfast = rb\n\
             [hash 0123456789abcdef]  # one hack\na = east\n\
             [title POKEMON EMER]\nfast =\n",
        )
        .unwrap();
        // The global map, for any other ROM, over the default.
        let other = c.mapping(&rom(1, "OTHER"));
        assert_eq!(other.controls(key::A), [SOUTH]);
        assert_eq!(other.controls(gamepad::FAST), [RB]);
        assert_eq!(other.controls(key::B), Mapping::default().controls(key::B));
        // The hack, by its hash: its own A, the global fast-forward.
        let hack = c.mapping(&EMERALD);
        assert_eq!(hack.controls(key::A), [EAST]);
        assert_eq!(hack.controls(gamepad::FAST), [RB]);
        // Any ROM with the title: no fast-forward; the hash beats it.
        let titled = c.mapping(&rom(2, "POKEMON EMER"));
        assert_eq!(titled.controls(key::A), [SOUTH]);
        assert!(titled.controls(gamepad::FAST).is_empty());
        let both = c.mapping(&rom(EMERALD.hash, "POKEMON EMER"));
        assert_eq!(both.controls(key::A), [EAST]);
        assert!(both.controls(gamepad::FAST).is_empty());
    }

    fn key(sc: Scancode) -> Control {
        Control::Key(sc)
    }

    #[test]
    fn a_roms_keyboard_override_wins_for_it_only() {
        let c = Config::parse(
            "key a = j k\nkey fast = left_shift\n\
             [hash 0123456789abcdef]\nkey a = space\nkey l =\n",
        )
        .unwrap();
        // The global keyboard map, for any other ROM, over the default;
        // the pads keep theirs.
        let other = c.mapping(&rom(1, "OTHER"));
        assert_eq!(other.keys(key::A), [key(Scancode::J), key(Scancode::K)]);
        assert_eq!(other.keys(gamepad::FAST), [key(Scancode::LShift)]);
        assert_eq!(other.keys(key::L), [key(Scancode::A)]);
        assert_eq!(other.controls(key::A), Mapping::default().controls(key::A));
        // The ROM's own section wins for it: Space is A, L has no key.
        let hack = c.mapping(&EMERALD);
        assert_eq!(hack.keys(key::A), [key(Scancode::Space)]);
        assert!(hack.keys(key::L).is_empty());
        assert_eq!(hack.keys(gamepad::FAST), [key(Scancode::LShift)]);
        assert_eq!(hack.controls(key::A), Mapping::default().controls(key::A));
        // Read as the window reads the keyboard.
        let ja = |sc| [Scancode::J, Scancode::A].contains(&sc);
        assert_eq!(hack.pressed(|sc| sc == Scancode::Space), key::A);
        assert_eq!(hack.pressed(ja), 0);
        assert_eq!(other.pressed(ja), key::A | key::L);
        // A pads' line and a keyboard's for one button are apart.
        let both = Config::parse("a = south\nkey a = j\n").unwrap();
        let m = both.mapping(&rom(1, ""));
        assert_eq!(
            (m.controls(key::A), m.keys(key::A)),
            (vec![SOUTH], vec![key(Scancode::J)])
        );
    }

    #[test]
    fn keys_are_named_as_sdl_names_them() {
        for sc in [
            Scancode::Grave,
            Scancode::LShift,
            Scancode::KpMinus,
            Scancode::NonUsHash,
        ] {
            let name = gamepad::control_name(key(sc));
            assert!(!name.contains([' ', '#']), "{name:?}");
            let line = format!("key a = {name}\n");
            let m = Config::parse(&line).unwrap().mapping(&rom(1, ""));
            assert_eq!(m.keys(key::A), [key(sc)], "{name:?}");
        }
        assert!(Config::parse("key a = nokey\n").is_err());
        assert!(Config::parse("key a = south\n").is_err());
        assert!(Config::parse("a = z\n").is_err());
    }

    #[test]
    fn adding_a_key_leaves_the_pads_alone() {
        let mut c = Controls {
            rom: emerald(),
            ..Controls::default()
        };
        // X presses A by default; added to B for this game, it's taken
        // from A, B keeps Z, and the pads' A and B are as they were.
        let taken = c.add(true, key::B, key(Scancode::X));
        assert_eq!(taken, [key::A]);
        let m = c.mapping();
        assert_eq!(m.keys(key::B), [key(Scancode::Z), key(Scancode::X)]);
        assert!(m.keys(key::A).is_empty());
        assert_eq!(m.controls(key::B), Mapping::default().controls(key::B));
        assert_eq!(
            c.config.text(),
            "[hash 0123456789abcdef]  # POKEMON EMER\nkey a =\nkey b = z x\n"
        );
        assert!(c.changed_here(key::B, true) && !c.changed_here(key::B, false));
        // Every game's is the default still.
        assert_eq!(c.view(false), Mapping::default());
        // Added again, nothing changes.
        let before = c.config.clone();
        assert!(c.add(true, key::B, key(Scancode::X)).is_empty());
        assert_eq!(c.config, before);
    }

    #[test]
    fn one_control_is_removed_and_this_game_goes_back_to_every_game() {
        let mut c = Controls {
            rom: emerald(),
            ..Controls::default()
        };
        // LT off L for every game: LB stays.
        let lt = Control::Stick(Axis::TriggerLeft, true);
        let lb = Control::Button(Button::LeftShoulder);
        c.remove(false, key::L, lt);
        assert_eq!(c.config.text(), "l = lb\n");
        assert_eq!(c.mapping().controls(key::L), [lb]);
        // Removing what isn't there changes nothing.
        c.remove(false, key::L, lt);
        assert_eq!(c.config.text(), "l = lb\n");
        // This game drops LB too, then uses every game's again.
        c.remove(true, key::L, lb);
        assert!(c.mapping().controls(key::L).is_empty());
        assert!(c.changed_here(key::L, false));
        c.use_every_game(key::L, false);
        assert!(!c.changed_here(key::L, false));
        assert_eq!(c.mapping().controls(key::L), [lb]);
        // Reset: this game's lines go, then every game's.
        c.add(true, key::A, RB);
        c.reset(true, false);
        assert_eq!(c.mapping(), c.view(false));
        c.reset(false, false);
        assert_eq!(c.mapping(), Mapping::default());
    }

    #[test]
    fn the_directions_are_whole_pads() {
        let mut c = Controls::default();
        // By default: the D-pad and the left stick; the arrow keys.
        let pads = Four::split(&c.view(false), false).unwrap();
        assert_eq!(
            pads.iter().map(|f| f.name).collect::<Vec<_>>(),
            ["D-pad", "left stick"]
        );
        let keys = Four::split(&c.view(false), true).unwrap();
        assert_eq!(keys, [&FOURS[3]]);
        // The right stick, added whole, is taken from rewind (its left).
        let right = Four::of(Control::Stick(Axis::RightX, true)).unwrap();
        assert_eq!(right.name, "right stick");
        assert_eq!(c.add_four(false, right), [gamepad::REWIND]);
        let pads = Four::split(&c.view(false), false).unwrap();
        assert_eq!(pads.len(), 3);
        // The left stick taken off whole.
        c.remove_four(false, pads[1]);
        assert_eq!(Four::split(&c.view(false), false).unwrap().len(), 2);
        // One direction apart: they're no longer whole pads.
        c.add(false, key::UP, Control::Button(Button::North));
        assert!(Four::split(&c.view(false), false).is_none());
        c.remove(false, key::UP, Control::Button(Button::North));
        c.remove(false, key::UP, Control::Stick(Axis::RightY, false));
        assert!(Four::split(&c.view(false), false).is_none());
        // None at all is no pads, not apart.
        let none = Config::parse("up =\ndown =\nleft =\nright =\n").unwrap();
        assert_eq!(
            Four::split(&none.mapping(&rom(1, "")), false),
            Some(Vec::new())
        );
    }

    #[test]
    fn bad_sections_are_refused() {
        for text in [
            "[hash 12]\n",
            "[rom x]\n",
            "[title ]\n",
            "[title A]\n[title A]\n",
            "[hash 0123456789abcdef]\na = nowhere\n",
        ] {
            assert!(Config::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn edits_keep_the_rest_of_the_file() {
        let text = "# june's\na = south\n\n# off for now\nsave-state =\n\n[title X]\nb = west\n";
        let mut c = Config::parse(text).unwrap();
        c.set(&Section::All, key::A, false, Some(&[EAST]));
        c.set(&Section::All, key::L, false, Some(&[RB]));
        assert_eq!(
            c.text(),
            "# june's\na = east\n\n# off for now\nsave-state =\nl = rb\n\n[title X]\nb = west\n"
        );
        // A new section goes at the end; dropping its only line leaves its
        // header.
        c.set(&EMERALD_SECTION, key::B, false, Some(&[]));
        assert!(
            c.text()
                .ends_with("b = west\n\n[hash 0123456789abcdef]\nb =\n")
        );
        assert!(c.mapping(&EMERALD).controls(key::B).is_empty());
        c.set(&EMERALD_SECTION, key::B, false, None);
        assert!(c.text().ends_with("b = west\n\n[hash 0123456789abcdef]\n"));
        assert_eq!(c.mapping(&EMERALD), c.mapping(&rom(5, "")));
        assert_eq!(Config::parse(&c.text()).unwrap(), c);
    }

    const EMERALD_SECTION: Section = Section::Hash(0x0123_4567_89ab_cdef);

    #[test]
    fn adding_takes_the_control_from_other_buttons() {
        let mut c = Controls {
            rom: emerald(),
            ..Controls::default()
        };
        // The button labelled X presses B by default; added to A for this
        // game, it doesn't any more.
        let x = Control::Label(gamepad::Label::X);
        assert_eq!(c.add(true, key::A, x), [key::B]);
        let m = c.mapping();
        assert_eq!(
            m.controls(key::A),
            [
                Control::Label(gamepad::Label::A),
                Control::Label(gamepad::Label::Y),
                x
            ]
        );
        assert_eq!(m.controls(key::B), [Control::Label(gamepad::Label::B)]);
        assert_eq!(
            c.config.text(),
            "[hash 0123456789abcdef]  # POKEMON EMER\nb = label-b\na = label-a label-y label-x\n"
        );
        // Other games keep the default.
        assert_eq!(c.config.mapping(&rom(9, "")), Mapping::default());
    }

    #[test]
    fn a_roms_title_is_its_header() {
        let mut rom = vec![0u8; 0x200];
        rom[0xA0..0xAC].copy_from_slice(b"POKEMON EMER");
        let id = RomId::of(&rom);
        assert_eq!(id.title, "POKEMON EMER");
        assert_eq!(id.hash, runtime::dispatch::rom_hash(&rom));
        rom[0xA5..0xAC].fill(0);
        assert_eq!(RomId::of(&rom).title, "POKEM");
    }
}
