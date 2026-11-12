//! Our own save states: all of [`Machine`]'s guest state at a frame
//! boundary, written and read back exactly, so a run from a state draws
//! what the run it was saved from went on to draw. mGBA's states
//! ([`crate::state`]) carry less (no BIOS waits in flight, no cartridge
//! save); these carry everything, the cartridge's save memory included.
//!
//! The file is a header, then the payload:
//!
//! | bytes | what |
//! |-------|------|
//! | 8 | [`MAGIC`] |
//! | 4 | [`VERSION`] |
//! | 4 | the ROM's CRC-32 ([`crate::state::crc32`]) |
//! | 4 | the payload's length |
//! | 4 | the payload's CRC-32 |
//!
//! The payload is every field of the machine in declaration order,
//! little-endian (`usize` as 8 bytes, `bool` as 1). Each type lists its
//! fields once, through [`snap!`], which destructures it without `..`: a
//! field added to a type and not to its list doesn't compile. Any change
//! to the payload bumps [`VERSION`] (a test pins its length). A state of
//! an older version back to [`OLDEST`] still loads: a field added since
//! asks [`Visitor::version`] and is skipped in older states, keeping its
//! default (`migrate` sets it); one of any other version is refused rather
//! than misread. A test loads a pinned state of each older version.
//!
//! Not in it: the ROM, the platform (the front end), the overlay guards
//! (rebuilt: every overlay checks its bytes again), [`Machine::fault`]
//! (states are saved only from a running machine) and [`Machine::timing`]
//! (a state loads into a machine of either).

use crate::Machine;
use crate::platform::Platform;
use armv4t::interp::Cpu;

/// What our states start with.
pub const MAGIC: [u8; 8] = *b"GBAAstat";
/// The payload's layout. Bump it with any change to what [`snap!`] lists.
pub const VERSION: u32 = 4;
/// The oldest version a state of ours may be and still load. Version 2
/// lacks the EEPROM's serial state (3), version 3 `MultiBoot`'s (4).
pub const OLDEST: u32 = 2;
/// The header's size.
pub const HEADER: usize = 24;

/// A pass over the fields: [`Saver`] copies each out, [`Loader`] each in.
pub trait Visitor {
    /// The next field's little-endian bytes: read them (saving) or
    /// overwrite them (loading).
    fn bytes(&mut self, b: &mut [u8]);

    /// The layout's version being saved or loaded: [`VERSION`] but when
    /// loading an older state. A field added in version `n` visits only
    /// when this is at least `n`.
    fn version(&self) -> u32 {
        VERSION
    }
}

/// A value that visits its fields. Saving leaves it as it was.
pub trait Snap {
    fn snap(&mut self, v: &mut dyn Visitor);
}

/// Implement [`Snap`] for a struct from its fields, listed in order, then
/// `; skip` and the fields left out, if any. Destructures without `..`, so
/// every field must be named.
macro_rules! snap {
    ($t:ident { $($f:ident),* $(,)? } $(; skip $($s:ident),+)?) => {
        impl $crate::snap::Snap for $t {
            fn snap(&mut self, v: &mut dyn $crate::snap::Visitor) {
                let $t { $($f,)* $($($s: _,)+)? } = self;
                $($crate::snap::Snap::snap($f, v);)*
            }
        }
    };
}
pub(crate) use snap;

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Snap for $t {
            fn snap(&mut self, v: &mut dyn Visitor) {
                let mut b = self.to_le_bytes();
                v.bytes(&mut b);
                *self = <$t>::from_le_bytes(b);
            }
        }
    )*};
}
int!(u8, u16, u32, u64, i8, i16, i32, i64);

impl Snap for usize {
    fn snap(&mut self, v: &mut dyn Visitor) {
        let mut n = *self as u64;
        n.snap(v);
        *self = n as usize;
    }
}

impl Snap for bool {
    fn snap(&mut self, v: &mut dyn Visitor) {
        let mut b = [*self as u8];
        v.bytes(&mut b);
        *self = b[0] != 0;
    }
}

impl<T: Snap, const N: usize> Snap for [T; N] {
    fn snap(&mut self, v: &mut dyn Visitor) {
        for x in self {
            x.snap(v);
        }
    }
}

impl<A: Snap, B: Snap> Snap for (A, B) {
    fn snap(&mut self, v: &mut dyn Visitor) {
        self.0.snap(v);
        self.1.snap(v);
    }
}

/// A flag, then the value (its default when there's none).
impl<T: Snap + Default> Snap for Option<T> {
    fn snap(&mut self, v: &mut dyn Visitor) {
        let mut some = self.is_some();
        some.snap(v);
        let mut x = self.take().unwrap_or_default();
        x.snap(v);
        *self = some.then_some(x);
    }
}

/// Byte arrays in one piece, rather than a call a byte.
pub(crate) fn bytes(b: &mut [u8], v: &mut dyn Visitor) {
    v.bytes(b);
}

impl Snap for Cpu {
    fn snap(&mut self, v: &mut dyn Visitor) {
        let Cpu {
            r,
            pc,
            cpsr,
            banks,
            spsr,
        } = self;
        r.snap(v);
        pc.snap(v);
        cpsr.snap(v);
        banks.snap(v);
        spsr.snap(v);
    }
}

/// Saving: each field's bytes to a sink.
struct Saver<'a>(&'a mut dyn FnMut(&[u8]));

impl Visitor for Saver<'_> {
    fn bytes(&mut self, b: &mut [u8]) {
        (self.0)(b);
    }
}

/// Counting the payload's bytes, and their CRC-32.
#[derive(Default)]
struct Counter {
    len: usize,
    crc: u32,
}

impl Visitor for Counter {
    fn bytes(&mut self, b: &mut [u8]) {
        self.len += b.len();
        self.crc = crc32_update(self.crc, b);
    }
}

/// Counting the payload's bytes alone, in a layout's version.
struct Length(usize, u32);

impl Visitor for Length {
    fn bytes(&mut self, b: &mut [u8]) {
        self.0 += b.len();
    }

    fn version(&self) -> u32 {
        self.1
    }
}

/// Loading: each field's bytes from the payload, of a layout's version,
/// which is known to be long enough.
struct Loader<'a>(&'a [u8], u32);

impl Visitor for Loader<'_> {
    fn bytes(&mut self, b: &mut [u8]) {
        let (now, rest) = self.0.split_at(b.len());
        b.copy_from_slice(now);
        self.0 = rest;
    }

    fn version(&self) -> u32 {
        self.1
    }
}

/// Why one of our states didn't load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapError {
    /// Doesn't start with [`MAGIC`].
    NotOurs,
    /// Written by a build with a payload layout this one can't read
    /// (newer, or older than [`OLDEST`]).
    Version(u32),
    /// Saved from another ROM (the CRC-32s).
    OtherRom { state: u32, rom: u32 },
    /// Cut short, too long, or its bytes don't match their CRC-32.
    Damaged,
}

impl core::fmt::Display for SnapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SnapError::NotOurs => write!(f, "not a gbaa save state"),
            SnapError::Version(v) => write!(
                f,
                "a gbaa save state of version {v}; this build reads versions {OLDEST} to {VERSION}"
            ),
            SnapError::OtherRom { state, rom } => write!(
                f,
                "saved from another ROM (CRC-32 {state:08x}, this ROM's is {rom:08x})"
            ),
            SnapError::Damaged => write!(f, "a damaged gbaa save state (length or CRC-32 wrong)"),
        }
    }
}

/// Whether `s` is one of our states (of any version), not mGBA's.
pub fn is_ours(s: &[u8]) -> bool {
    s.starts_with(&MAGIC)
}

/// The CRC-32 of the ROM our state `s` was saved from, if it is ours and
/// has its header.
pub fn state_rom(s: &[u8]) -> Option<u32> {
    let w = s.get(12..16).filter(|_| is_ours(s))?;
    Some(u32::from_le_bytes(w.try_into().unwrap()))
}

impl<P: Platform> Machine<P> {
    /// Visit the guest state: everything but the ROM, the platform, the
    /// overlay guards, the fault, the dispatcher's nesting and the timing;
    /// the picture too if `picture`.
    fn snap_fields(&mut self, v: &mut dyn Visitor, picture: bool) {
        let Machine {
            cpu,
            bus,
            bios,
            ppu,
            platform: _,
            sched,
            fault: _,
            prefetch,
            nest: _,
            timing: _,
            spin_list: _,
            low_sp: _,
        } = self;
        cpu.snap(v);
        bus.snap(v);
        bios.snap(v);
        if picture {
            ppu.snap(v);
        } else {
            ppu.snap_unseen(v);
        }
        sched.snap(v);
        prefetch.snap(v);
    }

    /// Write the state, header first, to `out` in pieces. Call it between
    /// frames. The machine is left as it was (`&mut` only because one pass
    /// serves saving and loading).
    pub fn save_state(&mut self, out: &mut dyn FnMut(&[u8])) {
        let mut c = Counter::default();
        self.snap_fields(&mut c, true);
        out(&MAGIC);
        out(&VERSION.to_le_bytes());
        out(&self.bus.rom_crc().to_le_bytes());
        out(&(c.len as u32).to_le_bytes());
        out(&c.crc.to_le_bytes());
        self.snap_fields(&mut Saver(out), true);
    }

    /// Replace the guest state with our state `s`, saved from this
    /// machine's ROM by a build of this [`VERSION`] or an older one back to
    /// [`OLDEST`]. Nothing is changed if it fails.
    pub fn load_state(&mut self, s: &[u8]) -> Result<(), SnapError> {
        self.load_state_from(s, false)
    }

    /// [`Machine::load_state`], but a state saved from another ROM (another
    /// version of the game) loads too: the front end's choice, having kept
    /// a copy of the file ([`state_rom`] tells it to).
    pub fn load_state_any_rom(&mut self, s: &[u8]) -> Result<(), SnapError> {
        self.load_state_from(s, true)
    }

    fn load_state_from(&mut self, s: &[u8], any_rom: bool) -> Result<(), SnapError> {
        if !is_ours(s) {
            return Err(SnapError::NotOurs);
        }
        let word = |o: usize| {
            s.get(o..o + 4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .ok_or(SnapError::Damaged)
        };
        let version = word(8)?;
        if !(OLDEST..=VERSION).contains(&version) {
            return Err(SnapError::Version(version));
        }
        let rom = self.bus.rom_crc();
        let state = word(12)?;
        if state != rom && !any_rom {
            return Err(SnapError::OtherRom { state, rom });
        }
        let payload = &s[HEADER.min(s.len())..];
        let mut n = Length(0, version);
        self.snap_fields(&mut n, true);
        if word(16)? as usize != n.0 || payload.len() != n.0 {
            return Err(SnapError::Damaged);
        }
        if crc32_update(0, payload) != word(20)? {
            return Err(SnapError::Damaged);
        }
        self.load_fields(payload, true, version)?;
        self.migrate(version);
        Ok(())
    }

    /// After loading a state of an older `version`: give the fields it
    /// lacked their defaults, rather than keep what the machine had.
    fn migrate(&mut self, version: u32) {
        if version < 3 {
            self.bus.save.reset_eeprom();
        }
        if version < 4 {
            self.bios.multiboot = crate::bios::multiboot::MultiBoot::default();
        }
    }

    /// The state's payload alone, without the header, for a front end's
    /// own keeping (rewind) where the bytes never leave this machine: no
    /// CRC-32 of the ROM or of the payload, so it's cheap every frame.
    pub fn save_payload(&mut self, out: &mut dyn FnMut(&[u8])) {
        self.snap_fields(&mut Saver(out), true);
    }

    /// Load a payload [`Machine::save_payload`] wrote from this machine.
    /// Nothing is changed if its length is wrong.
    pub fn load_payload(&mut self, payload: &[u8]) -> Result<(), SnapError> {
        self.load_fields(payload, true, VERSION)
    }

    /// [`Machine::save_payload`] without the picture ([`Ppu::frame`]), which
    /// nothing the guest does reads: what rollback keeps and netplay hashes,
    /// so a frame run again undrawn ([`Ppu::draw`]) matches one drawn.
    ///
    /// [`Ppu::frame`]: crate::Ppu::frame
    /// [`Ppu::draw`]: crate::Ppu::draw
    pub fn save_unseen(&mut self, out: &mut dyn FnMut(&[u8])) {
        self.snap_fields(&mut Saver(out), false);
    }

    /// Load a payload [`Machine::save_unseen`] wrote from this machine,
    /// leaving the picture as it is. Nothing is changed if its length is
    /// wrong.
    pub fn load_unseen(&mut self, payload: &[u8]) -> Result<(), SnapError> {
        self.load_fields(payload, false, VERSION)
    }

    fn load_fields(
        &mut self,
        payload: &[u8],
        picture: bool,
        version: u32,
    ) -> Result<(), SnapError> {
        let mut n = Length(0, version);
        self.snap_fields(&mut n, picture);
        if payload.len() != n.0 {
            return Err(SnapError::Damaged);
        }
        self.snap_fields(&mut Loader(payload, version), picture);
        // Compiled IWRAM code checks its bytes again before it next runs.
        self.bus.overlays.touch_all();
        self.fault = None;
        // Not saved: a frame's end is where IRQs may be taken.
        self.sched.window = true;
        self.sched.fresh = false;
        self.sched.spins = [(0, 0); crate::sched::SPIN_SLOTS];
        Ok(())
    }
}

/// zlib's CRC-32, continued from `crc` (0 to start).
fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    crate::state::crc32_continue(crc, data)
}

#[cfg(test)]
mod tests;
